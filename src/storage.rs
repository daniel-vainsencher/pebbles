use anyhow::{Context, Result};
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

    pub fn save_index(&self, index: &Index) -> Result<()> {
        let mut content =
            serde_json::to_string_pretty(index).context("Failed to serialize index")?;
        content.push('\n');
        fs::write(&self.index_path, content).context("Failed to write index file")?;
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

    // r[impl rename-commits-all-paths]
    pub fn save_issue(&self, issue: &Issue, index: &mut Index) -> Result<SaveOutcome> {
        self.ensure_issues_dir()?;

        let new_file_name = issue.file_name();
        let old_file_name = index.get_file_name(&issue.id).map(|s| s.to_string());

        let mut renamed_from: Option<String> = None;
        let mut index_modified = false;

        // Handle file rename if title or type changed
        if let Some(old_name) = old_file_name.as_deref() {
            if old_name != new_file_name {
                let old_path = self.issue_path(old_name);
                if old_path.exists() {
                    fs::remove_file(&old_path).context("Failed to remove old issue file")?;
                }
                index.update_file_name(&issue.id, new_file_name.clone());
                renamed_from = Some(old_name.to_string());
                index_modified = true;
            }
        } else {
            // New issue
            index.add(issue.id, new_file_name.clone());
            index_modified = true;
        }

        let path = self.issue_path(&new_file_name);
        let mut content =
            serde_json::to_string_pretty(issue).context("Failed to serialize issue")?;
        content.push('\n');
        fs::write(&path, content)
            .with_context(|| format!("Failed to write issue file: {}", new_file_name))?;

        Ok(SaveOutcome {
            new_file_name,
            renamed_from,
            index_modified,
        })
    }

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
    pub fn load_all_issues(&self) -> Result<Vec<Issue>> {
        let index = self.load_index()?;
        let mut issues = Vec::new();

        for metadata in &index.issues {
            let path = self.issue_path(&metadata.file_name);
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Issue>(&content) {
                    Ok(issue) => issues.push(issue),
                    Err(e) => eprintln!(
                        "Warning: Failed to parse issue {}: {}",
                        metadata.file_name, e
                    ),
                },
                Err(e) => eprintln!(
                    "Warning: Failed to load issue {}: {}",
                    metadata.file_name, e
                ),
            }
        }

        Ok(issues)
    }
}
