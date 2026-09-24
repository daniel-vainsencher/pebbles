use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::{Index, Issue};

/// Result of `Storage::save_issue`. Tells the caller which paths the operation
/// actually touched on disk so they can be passed to `JJ::commit`.
pub struct SaveOutcome {
    /// File name (relative to `.pebbles/issues/`) the issue is now stored under.
    pub new_file_name: String,
    /// If the operation renamed the issue file (title or type changed),
    /// the previous file name. The old file has been removed from disk;
    /// callers must include `issues_dir/<that>` in the commit path set.
    pub renamed_from: Option<String>,
    /// Whether `.pebbles/index.json` was modified. True for create or rename;
    /// false for in-place updates that don't change the filename.
    pub index_modified: bool,
}

/// The rename/index-dirty decision `Storage::save_issue` applies when
/// recording an issue under its new file name: whether the previous file
/// must be removed (rename) and whether `.pebbles/index.json` must be
/// rewritten. Pure data, produced by `plan_save_issue`.
pub struct SavePlan {
    /// Previous file name when the write renames the issue file (title or
    /// type changed); the old file must be removed from disk.
    pub renamed_from: Option<String>,
    /// Whether `.pebbles/index.json` was modified (new issue or rename).
    pub index_modified: bool,
}

/// Decide how `save_issue` should record an issue stored under
/// `new_file_name` in `index`, applying the bookkeeping to `index`:
///
/// - No entry for `issue_id`: a new issue; the index gains an entry and is
///   dirty (`index_modified`).
/// - An entry with a different file name: a rename (title or type
///   changed); the index entry is updated and is dirty.
/// - An entry with the same file name: an in-place update; the index is
///   unchanged.
///
/// Pure: no filesystem access. The file removal and write happen in
/// `Storage::save_issue` from the returned plan.
///
/// ```rust
/// # use pebbles::models::Index;
/// # use pebbles::storage::plan_save_issue;
/// # use uuid::Uuid;
/// let id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
/// let mut index = Index::new();
///
/// // A new issue: the index gains the entry and is dirty.
/// let plan = plan_save_issue(&mut index, id, "bug-11111111-x.json");
/// assert!(plan.index_modified);
/// assert_eq!(plan.renamed_from, None);
///
/// // An update under the same file name: nothing changes.
/// let plan = plan_save_issue(&mut index, id, "bug-11111111-x.json");
/// assert!(!plan.index_modified);
/// assert_eq!(plan.renamed_from, None);
///
/// // A rename (title or type changed): the old file must be removed.
/// let plan = plan_save_issue(&mut index, id, "bug-11111111-y.json");
/// assert!(plan.index_modified);
/// assert_eq!(plan.renamed_from, Some("bug-11111111-x.json".to_string()));
/// assert_eq!(index.get_file_name(&id), Some("bug-11111111-y.json"));
/// ```
pub fn plan_save_issue(index: &mut Index, issue_id: Uuid, new_file_name: &str) -> SavePlan {
    let old_name = index.get_file_name(&issue_id).map(|s| s.to_string());
    match old_name.as_deref() {
        Some(old) if old != new_file_name => {
            index.update_file_name(&issue_id, new_file_name.to_string());
            SavePlan {
                renamed_from: old_name,
                index_modified: true,
            }
        }
        Some(_) => SavePlan {
            renamed_from: None,
            index_modified: false,
        },
        None => {
            index.add(issue_id, new_file_name.to_string());
            SavePlan {
                renamed_from: None,
                index_modified: true,
            }
        }
    }
}

/// A per-item warning produced while loading the issue set or while writing
/// a mutation: the affected `.pebbles` file name (empty when the warning is
/// not specific to one file) and the warning text, without the `Warning: `
/// prefix the CLI adds.
///
/// Warnings are data, never stderr: `Storage::load_all_issues` no longer
/// prints, so rendering stays in the caller - CLI commands print each item
/// through their own conventions, and the TUI folds them into its
/// message/warning line instead of corrupting the raw-mode alternate screen
/// (r[load-warnings-as-data]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadWarning {
    pub file_name: String,
    pub message: String,
}

impl LoadWarning {
    /// The full CLI warning line this item renders to.
    ///
    /// ```rust
    /// # use pebbles::storage::LoadWarning;
    /// let warning = LoadWarning {
    ///     file_name: "bug-11111111-x.json".to_string(),
    ///     message: "Failed to parse bug-11111111-x.json".to_string(),
    /// };
    /// assert_eq!(warning.cli_line(), "Warning: Failed to parse bug-11111111-x.json");
    /// ```
    pub fn cli_line(&self) -> String {
        format!("Warning: {}", self.message)
    }
}

/// Result of `Storage::reindex`: what the rebuilt index contains and whether
/// anything on disk changed (r[reindex]).
pub struct ReindexOutcome {
    /// Number of issues in the rebuilt index.
    pub entries: usize,
    /// One entry per issue file that could not be read or parsed,
    /// `"<file name>: <reason>"`.
    pub unparsable: Vec<String>,
    /// File names skipped because they duplicate the id of an earlier file.
    pub duplicates: Vec<String>,
    /// Whether `.pebbles/index.json` was rewritten.
    pub wrote: bool,
}

/// The exactly-known top-level keys of the on-disk `Issue` schema. Any other
/// key is a schema violation (r[schema-valid-issues]): reported at load,
/// never silently dropped or preserved.
pub(crate) const KNOWN_ISSUE_KEYS: [&str; 10] = [
    "id",
    "title",
    "description",
    "status",
    "issue_type",
    "tags",
    "relationships",
    "metadata",
    "created_at",
    "updated_at",
];

/// The top-level keys of an issue file's JSON object that are not part of
/// the `Issue` schema. Conflicts with serde's accept-and-ignore default are
/// detected here so the load path can warn instead of silently accepting
/// (and later destroying on rewrite). Returns keys in sorted order; empty
/// when the content is not a JSON object (the caller treats non-object
/// content as a parse failure, r[schema-valid-issues]).
pub(crate) fn unknown_top_level_keys(content: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Vec::new();
    };
    match value {
        Value::Object(ref map) => map
            .keys()
            .filter(|key| !KNOWN_ISSUE_KEYS.iter().any(|known| *known == key.as_str()))
            .map(|key| key.clone())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    }
}

/// Order the scanned issue entries for the rebuilt index (r[reindex]):
/// entries already present in the old index keep the old index's order;
/// entries new to the index follow, in scan order (alphabetical by file
/// name). File names always come from the scan - the on-disk files are the
/// truth - so an issue file renamed outside pebbles is reindexed under its
/// current name. Deterministic when the old index is unparsable.
pub(crate) fn ordered_entries(
    entries: Vec<(Uuid, String)>,
    old_index: Option<&Index>,
) -> Vec<(Uuid, String)> {
    let Some(old) = old_index else {
        return entries;
    };
    let mut by_id: HashMap<Uuid, String> = HashMap::new();
    for (id, file_name) in &entries {
        by_id.insert(*id, file_name.clone());
    }
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for metadata in &old.issues {
        if let Some(file_name) = by_id.get(&metadata.id).cloned() {
            ordered.push((metadata.id, file_name));
            seen.insert(metadata.id);
        }
    }
    for (id, file_name) in &entries {
        if seen.insert(*id) {
            ordered.push((*id, file_name.clone()));
        }
    }
    ordered
}

pub struct Storage {
    pebbles_dir: PathBuf,
    issues_dir: PathBuf,
    index_path: PathBuf,
}

impl Storage {
    pub fn new(repo_path: &Path) -> Result<Self> {
        // Allow overriding pebbles directory via environment variable (for testing)
        let pebbles_dir = if let Ok(custom_path) = env::var("PEBBLES_DB_PATH") {
            PathBuf::from(custom_path)
        } else {
            repo_path.join(".pebbles")
        };
        let issues_dir = pebbles_dir.join("issues");
        let index_path = pebbles_dir.join("index.json");

        Ok(Self {
            pebbles_dir,
            issues_dir,
            index_path,
        })
    }

    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.issues_dir)
            .context("Failed to create .pebbles/issues directory")?;

        if !self.index_path.exists() {
            let index = Index::new();
            self.save_index(&index)?;
        }

        Ok(())
    }

    /// Whether the store is "already initialized": the index exists. This
    /// check is deliberately not the whole story for `init`, which must also
    /// distinguish the empty store from a lost index (r[init-lost-index]).
    pub fn exists(&self) -> bool {
        self.pebbles_dir.exists() && self.index_path.exists()
    }

    /// Count `*.json` files under `.pebbles/issues/`, used to distinguish the
    /// empty-store and lost-index cases before `init` (SPEC r[init-lost-index]).
    ///
    /// Returns `None` when the issues directory does not exist at all
    /// (nothing can be orphaned); `Some(n)` otherwise.
    pub fn issue_file_count(&self) -> Result<Option<usize>> {
        if !self.issues_dir.exists() {
            return Ok(None);
        }
        let entries =
            fs::read_dir(&self.issues_dir).context("Failed to list .pebbles/issues directory")?;
        let mut count = 0;
        for entry in entries {
            let entry = entry.context("Failed to read .pebbles/issues directory entry")?;
            if entry.file_name().to_string_lossy().ends_with(".json") {
                count += 1;
            }
        }
        Ok(Some(count))
    }

    pub fn issue_path(&self, file_name: &str) -> PathBuf {
        self.issues_dir.join(file_name)
    }

    pub fn index_path(&self) -> &PathBuf {
        &self.index_path
    }

    pub fn load_index(&self) -> Result<Index> {
        if !self.index_path.exists() {
            return Ok(Index::new());
        }

        let content = fs::read_to_string(&self.index_path).context("Failed to read index file")?;
        let index: Index = serde_json::from_str(&content).context("Failed to parse index file")?;
        Ok(index)
    }

    /// Write `content` to `path` atomically: into a uniquely-named temp file
    /// in the same directory, then rename over the target. Readers never
    /// observe a torn file, and a crashed writer leaves at worst a stray
    /// temp file (r[index-write-atomicity]).
    fn atomic_write(&self, path: &PathBuf, content: &str) -> Result<()> {
        let dir = path.parent().expect("store paths live in a directory");
        let tmp = dir.join(format!(".index-tmp-{}", Uuid::new_v4()));
        match fs::write(&tmp, content) {
            Ok(()) => match fs::rename(&tmp, path) {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = fs::remove_file(&tmp).ok();
                    Err(anyhow::anyhow!(
                        "Failed to write {}: {}",
                        path.to_string_lossy(),
                        e
                    ))
                }
            },
            Err(e) => {
                let _ = fs::remove_file(&tmp).ok();
                Err(anyhow::anyhow!(
                    "Failed to write {}: {}",
                    path.to_string_lossy(),
                    e
                ))
            }
        }
    }

    pub fn save_index(&self, index: &Index) -> Result<()> {
        let mut content =
            serde_json::to_string_pretty(index).context("Failed to serialize index")?;
        content.push('\n');
        self.atomic_write(&self.index_path, &content)?;
        Ok(())
    }

    /// Scan `.pebbles/issues/` for `*.json` files modified after the index
    /// version the current mutation read (baseline: the index file's mtime,
    /// not the clock) and not mapped by that index. Files this operation is
    /// writing itself are excluded. Returns a warning naming `pebbles
    /// reindex` when the index is stale (r[stale-index-warning]); the caller
    /// decides how to report it and continues the write either way.
    ///
    /// Only *unmapped* newer files count: a file the read index already maps
    /// under its name is not stale - the `id -> file_name` entry is still
    /// correct, even if a checkout or copy left the file mtime newer. The
    /// loss mode the check exists for (a concurrent writer whose issue file
    /// the index does not know) always leaves such a file behind.
    pub fn stale_index_warning(&self, own_files: &[&str]) -> Result<Option<LoadWarning>> {
        if !self.issues_dir.exists() {
            return Ok(None);
        }
        let Some(index) = self.load_index().ok() else {
            return Ok(None); // no index version was read; nothing to be stale against
        };
        let Some(baseline) = fs::metadata(&self.index_path)
            .and_then(|m| m.modified())
            .ok()
        else {
            return Ok(None);
        };
        let mut known = HashSet::new();
        for metadata in &index.issues {
            known.insert(metadata.file_name.clone());
        }
        let Ok(entries) = fs::read_dir(&self.issues_dir) else {
            return Ok(None);
        };
        let mut stale: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json")
                || known.contains(&name)
                || own_files.iter().any(|own| *own == name)
            {
                continue;
            }
            let Some(modified) = fs::metadata(&entry.path()).and_then(|m| m.modified()).ok() else {
                continue;
            };
            if modified > baseline {
                stale.push(name);
            }
        }
        if stale.is_empty() {
            return Ok(None);
        }
        stale.sort();
        Ok(Some(LoadWarning {
            file_name: String::new(),
            message: format!(
                "Index is stale: {} is newer than the index version read and is not in the \
                 index; a concurrent writer may have added an issue. Run 'pebbles reindex' to \
                 rebuild the index from the issue files.",
                stale.join(", ")
            ),
        }))
    }

    /// Write an index through the stale-index check: detect issue files the
    /// current operation did not write that are newer than the index version
    /// read, append the warning to `warnings` (data, not stderr), then write
    /// the index atomically. The operation continues regardless
    /// (r[stale-index-warning]).
    pub(crate) fn save_index_stale_aware(
        &self,
        index: &Index,
        own_files: &[&str],
        warnings: &mut Vec<LoadWarning>,
    ) -> Result<()> {
        let stale = self.stale_index_warning(own_files)?;
        if let Some(warning) = stale {
            warnings.push(warning);
        }
        self.save_index(index)?;
        Ok(())
    }

    pub fn load_issue(&self, id: &Uuid) -> Result<Issue> {
        let index = self.load_index()?;
        let file_name = index
            .get_file_name(id)
            .ok_or_else(|| anyhow::anyhow!("Issue not found in index: {}", id))?;
        let path = self.issue_path(file_name);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read issue file: {}", file_name))?;
        let issue: Issue = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse issue file: {}", file_name))?;
        Ok(issue)
    }

    /// Create `.pebbles/issues/` when it is missing. Mutations call this
    /// before writing an issue file: the directory can vanish on its own
    /// (jj tracks files, not directories, so removing the last issue file
    /// deletes the directory) and must not abort later writes.
    // r[impl issues-dir-created-on-demand]
    fn ensure_issues_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.issues_dir)
            .context("Failed to create .pebbles/issues directory")?;
        Ok(())
    }

    /// Write `issue` to `.pebbles/issues/<file_name>` and record it in
    /// `index`, applying the rename/index-dirty decision from
    /// `plan_save_issue`: a changed file name also removes the old file
    /// from disk. The caller commits every touched path (including the old
    /// file on rename and `index.json` when dirty) via the returned
    /// `SaveOutcome` (r[impl rename-commits-all-paths]).
    // r[impl rename-commits-all-paths]
    pub fn save_issue(&self, issue: &Issue, index: &mut Index) -> Result<SaveOutcome> {
        self.ensure_issues_dir()?;

        let new_file_name = issue.file_name();
        let plan = plan_save_issue(index, issue.id, &new_file_name);

        // Handle file rename if title or type changed
        if let Some(old_name) = &plan.renamed_from {
            let old_path = self.issue_path(old_name);
            if old_path.exists() {
                fs::remove_file(&old_path).context("Failed to remove old issue file")?;
            }
        }

        let path = self.issue_path(&new_file_name);
        let mut content =
            serde_json::to_string_pretty(issue).context("Failed to serialize issue")?;
        content.push('\n');
        fs::write(&path, content)
            .with_context(|| format!("Failed to write issue file: {}", new_file_name))?;

        Ok(SaveOutcome {
            new_file_name,
            renamed_from: plan.renamed_from,
            index_modified: plan.index_modified,
        })
    }

    /// Remove the issue file for `id` from disk, looked up through the
    /// index's file name. A no-op when the file is already gone or the id
    /// is not indexed; the caller updates the index itself.
    pub fn delete_issue(&self, id: &Uuid, index: &Index) -> Result<()> {
        if let Some(file_name) = index.get_file_name(id) {
            let path = self.issue_path(file_name);
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete issue file: {}", file_name))?;
            }
        }
        Ok(())
    }

    // r[impl schema-valid-issues]
    // r[impl load-warnings-as-data]
    /// Load every indexed issue, returning the issues plus a per-item warning
    /// for every file that failed to read, failed to parse, or contains keys
    /// outside the `Issue` schema. Never prints: the caller (CLI or TUI)
    /// renders the warnings through its own channel.
    pub fn load_all_issues(&self) -> Result<(Vec<Issue>, Vec<LoadWarning>)> {
        let index = self.load_index()?;
        let mut issues = Vec::new();
        let mut warnings = Vec::new();

        for metadata in &index.issues {
            let path = self.issue_path(&metadata.file_name);
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Issue>(&content) {
                    Ok(issue) => {
                        let unknown_keys = unknown_top_level_keys(&content);
                        if !unknown_keys.is_empty() {
                            warnings.push(LoadWarning {
                                file_name: metadata.file_name.clone(),
                                message: format!(
                                    "Issue {} has unknown key(s) not in the schema: {}; the \
                                     file is loaded but the key(s) will be dropped on the next \
                                     rewrite. Fix the file to remove them.",
                                    metadata.file_name,
                                    unknown_keys.join(", ")
                                ),
                            });
                        }
                        issues.push(issue);
                    }
                    Err(e) => warnings.push(LoadWarning {
                        file_name: metadata.file_name.clone(),
                        message: format!("Failed to parse issue {}: {}", metadata.file_name, e),
                    }),
                },
                Err(e) => warnings.push(LoadWarning {
                    file_name: metadata.file_name.clone(),
                    message: format!("Failed to load issue {}: {}", metadata.file_name, e),
                }),
            }
        }

        Ok((issues, warnings))
    }

    /// Rebuild `.pebbles/index.json` from the issue files (r[reindex]): the
    /// sanctioned repair for a conflicted or desynchronized index. The new
    /// index contains every file with a parseable UUID `id` (first file wins
    /// per id); entry order is preserved from the current index when it
    /// parses, and is deterministic (alphabetical by file name) otherwise.
    /// Never touches the issue files. The rebuilt index is written atomically
    /// and verified with a load before the write. Reports each unparsable
    /// file and each duplicate id.
    pub fn reindex(&self) -> Result<ReindexOutcome> {
        if !self.pebbles_dir.exists() {
            bail!(
                "No pebbles store found at {}. Run 'pebbles init' first.",
                self.pebbles_dir.to_string_lossy()
            );
        }

        // Scan the issue files (sorted by file name for determinism),
        // classifying each into indexable / unparsable / duplicate-id.
        let mut file_names: Vec<String> = Vec::new();
        if self.issues_dir.exists() {
            let Ok(entries) = fs::read_dir(&self.issues_dir) else {
                bail!(
                    "Failed to list {} directory",
                    self.issues_dir.to_string_lossy()
                );
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".json") {
                    file_names.push(name);
                }
            }
        }
        file_names.sort();

        let mut entries: Vec<(Uuid, String)> = Vec::new();
        let mut unparsable: Vec<String> = Vec::new();
        let mut duplicates: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for file_name in &file_names {
            let path = self.issue_path(&file_name);
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Value>(&content) {
                    Ok(value) => match value.get("id") {
                        None => {
                            unparsable.push(format!("{}: missing 'id' field", file_name));
                            continue;
                        }
                        Some(id_value) => match id_value {
                            Value::String(id_str) => match Uuid::parse_str(&id_str) {
                                Ok(uuid) => {
                                    if !seen.insert(uuid) {
                                        duplicates.push(file_name.clone());
                                    } else {
                                        entries.push((uuid, file_name.clone()));
                                    }
                                }
                                Err(_) => unparsable
                                    .push(format!("{}: 'id' is not a UUID: {}", file_name, id_str)),
                            },
                            _ => unparsable.push(format!("{}: 'id' is not a string", file_name)),
                        },
                    },
                    Err(e) => unparsable.push(format!("{}: {}", file_name, e)),
                },
                Err(e) => unparsable.push(format!("{}: {}", file_name, e)),
            }
        }

        // Order: current index order for known ids when the index parses,
        // deterministic scan order otherwise (r[reindex]).
        let old_index = self.load_index().ok();
        let ordered = match old_index {
            Some(old_index) => ordered_entries(entries, Some(&old_index)),
            None => ordered_entries(entries, None),
        };

        let mut index = Index::new();
        for (id, file_name) in ordered {
            index.add(id, file_name);
        }
        let mut content =
            serde_json::to_string_pretty(&index).context("Failed to serialize index")?;
        content.push('\n');

        // Verify with a load: the serialized index must parse back as an
        // `Index` before it is written (r[reindex]).
        serde_json::from_str::<Index>(&content)
            .map_err(|e| anyhow::anyhow!("Reindex produced an unparsable index: {}", e))?;

        let needs_write = match fs::read_to_string(&self.index_path).ok() {
            Some(current) => Some(current.as_str()) != Some(content.as_str()),
            None => true,
        };

        if needs_write {
            self.atomic_write(&self.index_path, &content)?;
        }

        Ok(ReindexOutcome {
            entries: index.issues.len(),
            unparsable,
            duplicates,
            wrote: needs_write,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn index_with(entries: Vec<(&str, &str)>) -> Index {
        let mut index = Index::new();
        for (id, file_name) in entries {
            index.add(uuid(id), file_name.to_string());
        }
        index
    }

    #[test]
    fn ordered_entries_without_old_index_is_scan_order() {
        let entries = vec![
            (
                uuid("11111111-1111-1111-1111-111111111111"),
                "b.json".to_string(),
            ),
            (
                uuid("22222222-2222-2222-2222-222222222222"),
                "a.json".to_string(),
            ),
        ];
        assert_eq!(
            ordered_entries(entries.clone(), None)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            vec![
                uuid("11111111-1111-1111-1111-111111111111"),
                uuid("22222222-2222-2222-2222-222222222222"),
            ]
        );
    }

    #[test]
    fn ordered_entries_preserves_old_order_then_appends_new() {
        let old = index_with(vec![
            ("22222222-2222-2222-2222-222222222222", "a.json"),
            ("33333333-3333-3333-3333-333333333333", "c.json"),
        ]);
        let entries = vec![
            (
                uuid("11111111-1111-1111-1111-111111111111"),
                "b.json".to_string(),
            ),
            (
                uuid("22222222-2222-2222-2222-222222222222"),
                "a.json".to_string(),
            ),
            (
                uuid("33333333-3333-3333-3333-333333333333"),
                "c.json".to_string(),
            ),
        ];
        let ordered = ordered_entries(entries, Some(&old));
        assert_eq!(
            ordered.into_iter().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![
                uuid("22222222-2222-2222-2222-222222222222"),
                uuid("33333333-3333-3333-3333-333333333333"),
                uuid("11111111-1111-1111-1111-111111111111"),
            ]
        );
    }

    #[test]
    fn ordered_entries_takes_file_names_from_the_scan() {
        // The on-disk file names are the truth, even when the old index
        // names a different file for the same id.
        let old = index_with(vec![(
            "11111111-1111-1111-1111-111111111111",
            "old-name.json",
        )]);
        let entries = vec![(
            uuid("11111111-1111-1111-1111-111111111111"),
            "new-name.json".to_string(),
        )];
        let ordered = ordered_entries(entries, Some(&old));
        assert_eq!(
            ordered.into_iter().map(|(_, f)| f).collect::<Vec<_>>(),
            vec!["new-name.json".to_string()]
        );
    }

    #[test]
    fn unknown_top_level_keys_reports_only_unknowns() {
        let content = r#"{
            "id": "11111111-1111-1111-1111-111111111111",
            "title": "x",
            "priority": "high",
            "assignee": "daniel",
            "status": "todo"
        }"#;
        assert_eq!(
            unknown_top_level_keys(content),
            vec!["assignee".to_string(), "priority".to_string()]
        );
    }

    #[test]
    fn unknown_top_level_keys_clean_file_is_empty() {
        let content = r#"{
            "id": "11111111-1111-1111-1111-111111111111",
            "title": "x",
            "status": "todo",
            "metadata": {}
        }"#;
        assert!(unknown_top_level_keys(content).is_empty());
    }

    #[test]
    fn unknown_top_level_keys_non_object_is_empty() {
        assert!(unknown_top_level_keys("not json").is_empty());
        assert!(unknown_top_level_keys("[1, 2]").is_empty());
    }

    #[test]
    fn load_warning_cli_line_has_warning_prefix() {
        let warning = LoadWarning {
            file_name: "bug-11111111-x.json".to_string(),
            message: "Failed to parse issue bug-11111111-x.json: nope".to_string(),
        };
        assert_eq!(
            warning.cli_line(),
            "Warning: Failed to parse issue bug-11111111-x.json: nope"
        );
    }

    // -- plan_save_issue (pure rename/index-dirty decision) -------------------

    #[test]
    fn plan_save_issue_new_issue_adds_to_index_and_dirty() {
        let mut index = Index::new();
        let id = uuid("11111111-1111-1111-1111-111111111111");
        let plan = plan_save_issue(&mut index, id, "bug-11111111-x.json");
        assert_eq!(plan.renamed_from, None);
        assert!(plan.index_modified);
        assert_eq!(index.get_file_name(&id), Some("bug-11111111-x.json"));
    }

    #[test]
    fn plan_save_issue_same_name_is_in_place_no_rename_no_dirty() {
        let mut index = index_with(vec![(
            "11111111-1111-1111-1111-111111111111",
            "bug-11111111-x.json",
        )]);
        let id = uuid("11111111-1111-1111-1111-111111111111");
        let plan = plan_save_issue(&mut index, id, "bug-11111111-x.json");
        assert_eq!(plan.renamed_from, None);
        assert!(!plan.index_modified);
        assert_eq!(index.get_file_name(&id), Some("bug-11111111-x.json"));
    }

    #[test]
    fn plan_save_issue_changed_name_renames_and_dirty() {
        let mut index = index_with(vec![(
            "11111111-1111-1111-1111-111111111111",
            "bug-11111111-old.json",
        )]);
        let id = uuid("11111111-1111-1111-1111-111111111111");
        let plan = plan_save_issue(&mut index, id, "bug-11111111-new.json");
        assert_eq!(plan.renamed_from, Some("bug-11111111-old.json".to_string()));
        assert!(plan.index_modified);
        assert_eq!(index.get_file_name(&id), Some("bug-11111111-new.json"));
    }

    // -- reindex / stale-index (I/O-backed; tempdirs) ------------------------

    fn issue_id(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn write_json_file(path: &std::path::PathBuf, content: &str) {
        std::fs::write(path, content).expect("write fixture file");
    }

    #[test]
    fn reindex_rebuilds_index_from_files_and_reports_problems() {
        let tmp = tempfile::Builder::new()
            .prefix("pebbles-reindex-")
            .tempdir()
            .expect("tempdir");
        let root = tmp.path().to_path_buf();
        let storage = Storage::new(&root).unwrap();
        storage.init().unwrap();

        // Three valid files (scan order aaa, bbb, ccc), one unparsable file,
        // one duplicate of aaa.
        write_json_file(
            &storage.issue_path("aaa-a.json"),
            r#"{ "id": "11111111-1111-1111-1111-111111111111" }"#,
        );
        write_json_file(
            &storage.issue_path("bbb-b.json"),
            r#"{ "id": "22222222-2222-2222-2222-222222222222" }"#,
        );
        write_json_file(
            &storage.issue_path("ccc-c.json"),
            r#"{ "id": "33333333-3333-3333-3333-333333333333" }"#,
        );
        write_json_file(&storage.issue_path("zzz-broken.json"), "not json");
        write_json_file(
            &storage.issue_path("yyy-dup.json"),
            r#"{ "id": "11111111-1111-1111-1111-111111111111" }"#,
        );

        // The old index exists and is parseable, but its order differs from
        // scan order, and it does not include every file.
        let mut old = Index::new();
        old.add(
            issue_id("33333333-3333-3333-3333-333333333333"),
            "ccc-c.json".to_string(),
        );
        old.add(
            issue_id("11111111-1111-1111-1111-111111111111"),
            "aaa-a.json".to_string(),
        );
        storage.save_index(&old).unwrap();

        let unchanged = storage.reindex().unwrap();
        assert_eq!(unchanged.entries, 3);
        assert_eq!(unchanged.unparsable.len(), 1, "{:?}", unchanged.unparsable);
        assert!(
            unchanged.unparsable[0].contains("zzz-broken.json"),
            "{:?}",
            unchanged.unparsable
        );
        assert_eq!(unchanged.duplicates, vec!["yyy-dup.json".to_string()]);
        assert!(unchanged.wrote);

        // Old index order is preserved for known ids; new ids follow in scan
        // order; unparsable and duplicate files are excluded.
        let rebuilt = storage.load_index().unwrap();
        let order: Vec<Uuid> = rebuilt.issues.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            order,
            vec![
                issue_id("33333333-3333-3333-3333-333333333333"),
                issue_id("11111111-1111-1111-1111-111111111111"),
                issue_id("22222222-2222-2222-2222-222222222222"),
            ]
        );

        // A second reindex is a no-op write on the healthy store.
        let again = storage.reindex().unwrap();
        assert_eq!(again.entries, 3);
        assert!(!again.wrote, "healthy store reindex must not rewrite");
    }

    #[test]
    fn stale_index_warning_flags_unindexed_newer_file() {
        let tmp = tempfile::Builder::new()
            .prefix("pebbles-stale-")
            .tempdir()
            .expect("tempdir");
        let root = tmp.path().to_path_buf();
        let storage = Storage::new(&root).unwrap();
        storage.init().unwrap();

        // A brand-new issue file written after the index and not in it.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write_json_file(
            &storage.issue_path("bug-11111111-orphan.json"),
            r#"{ "id": "11111111-1111-1111-1111-111111111111" }"#,
        );

        let Some(warning) = storage.stale_index_warning(&[]).unwrap() else {
            panic!("expected a stale-index warning");
        };
        assert!(warning.message.contains("reindex"), "{:?}", warning.message);
        assert!(
            warning.message.contains("bug-11111111-orphan.json"),
            "{:?}",
            warning.message
        );
    }

    #[test]
    fn stale_index_warning_ignores_indexed_files_and_own_files() {
        let tmp = tempfile::Builder::new()
            .prefix("pebbles-stale-")
            .tempdir()
            .expect("tempdir");
        let root = tmp.path().to_path_buf();
        let storage = Storage::new(&root).unwrap();
        storage.init().unwrap();

        // Indexed file: even a later rewrite (newer mtime) is not staleness -
        // the index entry id -> file_name is still correct.
        let mut index = storage.load_index().unwrap();
        index.add(
            issue_id("11111111-1111-1111-1111-111111111111"),
            "bug-11111111-x.json".to_string(),
        );
        storage.save_index(&index).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        write_json_file(
            &storage.issue_path("bug-11111111-x.json"),
            r#"{ "id": "11111111-1111-1111-1111-111111111111", "status": "todo" }"#,
        );
        assert!(
            storage.stale_index_warning(&[]).unwrap().is_none(),
            "indexed file rewrite must not flag staleness"
        );

        // And an unindexed newer file that the operation itself wrote is
        // excluded by name.
        write_json_file(
            &storage.issue_path("bug-22222222-own.json"),
            r#"{ "id": "22222222-2222-2222-2222-222222222222" }"#,
        );
        assert!(
            storage
                .stale_index_warning(&["bug-22222222-own.json"])
                .unwrap()
                .is_none(),
            "own file must be excluded from the stale check"
        );
    }
}
