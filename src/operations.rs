use anyhow::{bail, Context, Result};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::jj::JJ;
use crate::models::{
    build_dependency_adjacency, build_epic_counts, build_incoming_relationships_map, creates_cycle,
    Issue, IssueType, RelationType, Status,
};
use crate::storage::{LoadWarning, ReindexOutcome, SaveOutcome, Storage};

/// Aggregated view of an issue set for `pebbles summary`: totals by
/// status and type, per-epic child progress, the actionable (unblocked
/// todo) count, and relationship totals. Produced by `calculate_summary`.
pub struct ProjectSummary {
    pub total_issues: usize,
    pub by_status: HashMap<Status, usize>,
    pub by_type: HashMap<IssueType, usize>,
    pub epics: Vec<(Issue, usize, usize)>, // (epic, total_children, done_children)
    pub actionable_count: usize,
    pub total_relationships: usize,
    pub by_relationship_type: HashMap<RelationType, usize>,
}

/// Result of a mutating operation: the operation's own value plus any
/// warnings produced while writing (a stale-index warning naming
/// `pebbles reindex`, or the store-without-jj note). Warnings are data, not
/// stderr: the CLI and the TUI each report them through their own channels
/// (r[load-warnings-as-data]).
pub struct MutationOutcome<T> {
    pub value: T,
    pub warnings: Vec<LoadWarning>,
}

/// Titles must be non-empty after trimming: they become slugs in filenames
/// and the primary human-readable handle for an issue.
fn validate_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        bail!("Issue title cannot be empty");
    }
    Ok(())
}

/// Extract candidate issue IDs from one line of stdin bulk input.
///
/// Bulk input is designed for Unix composition: it accepts the output of
/// the listing commands (`pebbles list`, `next`, `review`, `epic` — one
/// issue per line, ID as the first field) as well as bare IDs.
///
/// Rule per line:
/// - Blank lines contribute nothing.
/// - A line whose whitespace-separated fields are all ID-like (hex digits,
///   plus the dashes of a full UUID — e.g. `333 444` or two UUIDs) is
///   treated as several ID inputs.
/// - Any other line contributes only its first field, so a list row like
///   `333 [des] [TODO] JWT token authentication system` yields `333`.
///
/// ```rust
/// # use pebbles::operations::parse_id_input_line;
/// assert_eq!(parse_id_input_line("333 [bug] [DONE] Fix login"), vec!["333"]);
/// assert_eq!(parse_id_input_line("333 444 555"), vec!["333", "444", "555"]);
/// assert!(parse_id_input_line("   ").is_empty());
/// ```
pub fn parse_id_input_line(line: &str) -> Vec<&str> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.is_empty() {
        return Vec::new();
    }
    let all_id_like = fields
        .iter()
        .all(|f| f.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    if all_id_like {
        fields
    } else {
        vec![fields[0]]
    }
}

/// Resolve a full UUID or an unambiguous prefix against `known_ids`.
///
/// A full UUID that parses must be present in `known_ids` exactly (the
/// error names the UUID). Any other input is prefix-matched
/// case-insensitively against the lowercase UUID strings
/// (SPEC r[short-id-input]): exactly one match resolves, zero matches and
/// multiple matches are errors. Pure: no storage access, so the matching
/// logic is unit-testable without a filesystem (ARCHITECTURE: functional
/// core, imperative shell).
///
/// ```rust
/// # use pebbles::operations::match_id_prefix;
/// # use uuid::Uuid;
/// let a = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
/// let b = Uuid::parse_str("aaaaaaaa-1111-2222-3333-444444444444").unwrap();
/// let ids = vec![a, b];
///
/// // A full UUID resolves exactly; an unambiguous prefix resolves
/// // case-insensitively.
/// assert_eq!(match_id_prefix("11111111-2222-3333-4444-555555555555", &ids).unwrap(), a);
/// assert_eq!(match_id_prefix("AAAAAAA", &ids).unwrap(), b);
///
/// // A prefix shared by several ids is ambiguous and fails.
/// let c = Uuid::parse_str("11111111-9999-8888-7777-666666666666").unwrap();
/// assert!(match_id_prefix("11111111", &vec![a, c]).is_err());
///
/// // So does a full UUID that is not in the set.
/// let unknown = Uuid::parse_str("99999999-8888-7777-6666-555555555555").unwrap();
/// assert!(match_id_prefix(&unknown.to_string(), &ids).is_err());
/// ```
pub fn match_id_prefix(id_str: &str, known_ids: &[Uuid]) -> Result<Uuid> {
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        if known_ids.contains(&uuid) {
            return Ok(uuid);
        }
        bail!("Issue with ID {} not found", uuid);
    }

    // Prefix matching is case-insensitive (SPEC r[short-id-input]).
    let id_str = id_str.to_lowercase();
    let matches: Vec<_> = known_ids
        .iter()
        .filter(|uuid| uuid.to_string().starts_with(&id_str))
        .collect();

    match matches.len() {
        0 => bail!("No issue found with ID starting with '{}'", id_str),
        1 => Ok(*matches[0]),
        _ => bail!("Ambiguous ID '{}'. Multiple issues match.", id_str),
    }
}

pub struct Operations {
    storage: Storage,
    jj: JJ,
}

/// Build the list of paths to pass to `JJ::commit` based on what
/// `Storage::save_issue` did. Always includes the new issue file;
/// also includes the old issue file (deletion) on rename, and
/// `.pebbles/index.json` when the index was modified.
// r[impl jj-commit-complete-path-set]
fn paths_for_save(storage: &Storage, outcome: &SaveOutcome) -> Vec<PathBuf> {
    let mut paths = vec![storage.issue_path(&outcome.new_file_name)];
    if let Some(old) = &outcome.renamed_from {
        paths.push(storage.issue_path(old));
    }
    if outcome.index_modified {
        paths.push(storage.index_path().clone());
    }
    paths
}

/// Convert a `Vec<PathBuf>` to a `Vec<&Path>` for passing to `JJ::commit`.
fn as_path_refs(paths: &[PathBuf]) -> Vec<&Path> {
    paths.iter().map(|p| p.as_path()).collect()
}

/// The issues from `issues` that `pebbles next` shows: status `Todo` and
/// not blocked by an incoming `required_for` or `scheduled_before`
/// relationship from a non-Done issue (SPEC `next`). Sorted by
/// `updated_at` descending (r[impl issue-ordering]). Pure: no storage
/// access, so the blocking rules are unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType, RelationType, Relationship, Status};
/// # use pebbles::operations::get_unblocked_issues;
/// let mut blocker = Issue::new("blocker".to_string(), None, IssueType::Bug);
/// blocker.status = Status::Todo;
/// let blocked = Issue::new("blocked".to_string(), None, IssueType::Bug);
/// let blocker_id = blocker.id;
/// blocker.relationships.push(Relationship {
///     relation_type: RelationType::RequiredFor,
///     target_id: blocked.id,
/// });
/// let out = get_unblocked_issues(&[blocker, blocked]);
/// assert_eq!(out.len(), 1);
/// assert_eq!(out[0].id, blocker_id);
/// ```
pub fn get_unblocked_issues(issues: &[Issue]) -> Vec<Issue> {
    let incoming_map = build_incoming_relationships_map(issues);

    let mut unblocked_issues: Vec<Issue> = issues
        .iter()
        .filter(|issue| issue.status == Status::Todo)
        .filter(|issue| {
            if let Some(incoming) = incoming_map.get(&issue.id) {
                for rel in incoming {
                    if (rel.relation_type == RelationType::RequiredFor
                        || rel.relation_type == RelationType::ScheduledBefore)
                        && rel.source_status != Status::Done
                    {
                        return false;
                    }
                }
            }
            true
        })
        .cloned()
        .collect();

    // r[impl issue-ordering]
    unblocked_issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

    unblocked_issues
}

/// The issues from `issues` whose title fuzzy-matches `query`, sorted by
/// match score descending; equal scores break by `updated_at` descending,
/// keeping the established ordering rule (r[impl issue-ordering]) within a
/// relevance class.
///
/// Matching is fuzzy-matcher's `SkimMatcherV2`: case-insensitive
/// subsequence matching with word-boundary bonuses (a query does not have
/// to be a contiguous substring of the title). An empty (or whitespace
/// only) query matches nothing: `--search ""` is a distinct filter, not
/// the unfiltered list (r[continuity]).
///
/// Pure: no storage access, so the matching and ordering logic is
/// unit-testable without I/O (ARCHITECTURE: functional core, imperative
/// shell).
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType, Status};
/// # use pebbles::operations::filter_by_title_query;
/// let mut a = Issue::new("Login button".to_string(), None, IssueType::Bug);
/// let mut b = Issue::new("Export users to CSV".to_string(), None, IssueType::Bug);
/// let out = filter_by_title_query(&[a.clone(), b.clone()], "login");
/// assert_eq!(out.len(), 1);
/// assert_eq!(out[0].id, a.id);
/// ```
pub fn filter_by_title_query(issues: &[Issue], query: &str) -> Vec<Issue> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    // Always case-insensitive: a search for "LOGIN" finds "login" and
    // vice versa (the matcher default is smart-case, which would treat an
    // all-uppercase query as case-sensitive).
    let matcher = SkimMatcherV2::default().ignore_case();
    let mut scored: Vec<(i64, Issue)> = issues
        .iter()
        .filter_map(|issue| {
            matcher
                .fuzzy_match(&issue.title, query)
                .map(|score| (score, issue.clone()))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
    });
    scored.into_iter().map(|(_, issue)| issue).collect()
}

/// Aggregate `issues` into a `ProjectSummary`: total count, counts by
/// status and type, per-epic child progress (`part_of` children, done
/// count), the actionable (unblocked todo) count, and relationship totals
/// by relation type. Pure: no storage access, so the aggregation and the
/// actionability link to `get_unblocked_issues` are unit-testable without
/// I/O.
pub fn calculate_summary(issues: &[Issue]) -> ProjectSummary {
    let total_issues = issues.len();

    // Count by status
    let mut by_status: HashMap<Status, usize> = HashMap::new();
    for issue in issues {
        *by_status.entry(issue.status.clone()).or_insert(0) += 1;
    }

    // Count by type
    let mut by_type: HashMap<IssueType, usize> = HashMap::new();
    for issue in issues {
        *by_type.entry(issue.issue_type.clone()).or_insert(0) += 1;
    }

    // Gather epic info
    let epics: Vec<Issue> = issues
        .iter()
        .filter(|i| i.issue_type == IssueType::Epic)
        .cloned()
        .collect();

    let mut epic_info = Vec::new();
    for epic in epics {
        let children: Vec<&Issue> = issues
            .iter()
            .filter(|issue| {
                issue.relationships.iter().any(|rel| {
                    rel.relation_type == RelationType::PartOf && rel.target_id == epic.id
                })
            })
            .collect();

        let total_children = children.len();
        let done_children = children.iter().filter(|i| i.status == Status::Done).count();

        epic_info.push((epic, total_children, done_children));
    }

    // Count actionable issues
    let actionable_count = get_unblocked_issues(issues).len();

    // Count relationships
    let mut total_relationships = 0;
    let mut by_relationship_type: HashMap<RelationType, usize> = HashMap::new();
    for issue in issues {
        for rel in &issue.relationships {
            total_relationships += 1;
            *by_relationship_type
                .entry(rel.relation_type.clone())
                .or_insert(0) += 1;
        }
    }

    ProjectSummary {
        total_issues,
        by_status,
        by_type,
        epics: epic_info,
        actionable_count,
        total_relationships,
        by_relationship_type,
    }
}

impl Operations {
    pub fn new(repo_path: &Path) -> Result<Self> {
        let storage = Storage::new(repo_path)?;
        let jj = JJ::new(repo_path)?;

        Ok(Self { storage, jj })
    }

    // r[impl init-lost-index]
    pub fn init(&self, force: bool) -> Result<()> {
        if self.storage.exists() {
            bail!("Pebbles is already initialized in this repository");
        }

        // Distinguish the empty store from a lost index: a store whose
        // issues directory holds files while index.json is missing would be
        // left with every issue silently orphaned by a fresh empty index.
        // Refuse unless --force makes the orphan explicit.
        if !force {
            if let Some(count) = self.storage.issue_file_count()? {
                if count > 0 {
                    bail!(
                        ".pebbles/index.json is missing but .pebbles/issues/ contains {} issue file(s); \
                         initializing now would write a fresh empty index and silently orphan every \
                         issue. Rebuild the index from the issue files with 'pebbles reindex', or \
                         pass --force to create a fresh empty index anyway.",
                        count
                    );
                }
            }
        }

        if !self.jj.is_repo() {
            self.jj
                .init()
                .context("Failed to initialize jj repository")?;
        }

        self.storage
            .init()
            .context("Failed to initialize pebbles storage")?;
        self.jj
            .commit("Initialize pebbles", &[self.storage.index_path().as_path()])?;

        println!("✓ Pebbles initialized successfully");
        Ok(())
    }

    // r[impl new-issues-default-proposed]
    pub fn create(
        &self,
        title: String,
        description: Option<String>,
        issue_type: IssueType,
        approved: bool,
    ) -> Result<MutationOutcome<Issue>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        validate_title(&title)?;

        // New issues start as proposed; `--approved` skips review and
        // creates them directly as todo.
        let mut issue = Issue::new(title, description, issue_type);
        if approved {
            issue.status = Status::Todo;
        }

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!("Create {}: {}", issue.issue_type, issue.title),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: issue,
            warnings,
        })
    }

    // r[impl list-filter]
    pub fn list(
        &self,
        status_filter: Option<Status>,
        type_filter: Option<IssueType>,
    ) -> Result<(Vec<Issue>, Vec<LoadWarning>)> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let (mut issues, warnings) = self.storage.load_all_issues()?;

        if let Some(status) = status_filter {
            issues.retain(|issue| issue.status == status);
        }

        if let Some(issue_type) = type_filter {
            issues.retain(|issue| issue.issue_type == issue_type);
        }

        // r[impl issue-ordering]
        issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        Ok((issues, warnings))
    }

    pub fn show(&self, id: &str) -> Result<Issue> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        self.storage.load_issue(&uuid)
    }

    /// Returns `Ok(false)` when the issue already had the requested status
    /// (no file write, no jj commit).
    pub fn update_status(&self, id: &str, new_status: Status) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        if issue.status == new_status {
            return Ok(MutationOutcome {
                value: false,
                warnings: Vec::new(),
            });
        }

        let old_status = issue.status.clone();
        issue.status = new_status;
        issue.update();

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!(
                "Update {} {}: {} -> {}",
                issue.issue_type, issue.title, old_status, issue.status
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: true,
            warnings,
        })
    }

    // r[impl approve-command]
    pub fn approve(&self, id: &str) -> Result<MutationOutcome<()>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let issue = self.storage.load_issue(&uuid)?;

        if issue.status != Status::Proposed {
            let short = self.appropriate_length_id(&uuid)?;
            bail!(
                "Issue {} is not proposed (current status: {})",
                short,
                issue.status
            );
        }

        // Approving is a status change proposed -> todo, reusing the regular
        // status-update path (save + jj commit with the issue file path).
        let outcome = self.update_status(id, Status::Todo)?;
        Ok(MutationOutcome {
            value: (),
            warnings: outcome.warnings,
        })
    }

    /// Returns `Ok(false)` when nothing changed (no file write, no jj commit).
    pub fn update(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
    ) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let mut changed = false;

        if let Some(new_title) = title {
            validate_title(&new_title)?;
            if issue.title != new_title {
                issue.title = new_title;
                changed = true;
            }
        }

        if let Some(new_desc) = description {
            if issue.description.as_deref() != Some(new_desc.as_str()) {
                issue.description = Some(new_desc);
                changed = true;
            }
        }

        if !changed {
            return Ok(MutationOutcome {
                value: false,
                warnings: Vec::new(),
            });
        }

        issue.update();

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!("Update {}: {}", issue.issue_type, issue.title),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: true,
            warnings,
        })
    }

    /// Deletes the issue and returns its appropriate-length ID (computed
    /// while the index still contains it) for display purposes.
    ///
    /// r[impl no-broken-links]: if other issues reference this one, the
    /// delete fails naming each broken link unless `delete_links_too` is
    /// set, in which case the referencing relationships are removed in the
    /// same operation and commit.
    pub fn delete(&self, id: &str, delete_links_too: bool) -> Result<MutationOutcome<String>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let short = self.appropriate_length_id(&uuid)?;
        let issue = self.storage.load_issue(&uuid)?;
        let issue_path = self.storage.issue_path(&issue.file_name());

        // Find every issue with a relationship pointing at this one.
        let (all_issues, _) = self.storage.load_all_issues()?;
        let referencing: Vec<Issue> = all_issues
            .into_iter()
            .filter(|i| i.relationships.iter().any(|rel| rel.target_id == uuid))
            .collect();

        if !referencing.is_empty() && !delete_links_too {
            let listing = referencing
                .iter()
                .map(|i| {
                    let rel = i
                        .relationships
                        .iter()
                        .find(|rel| rel.target_id == uuid)
                        .expect("filtered to referencing issues");
                    format!(
                        "  {} '{}' ({})",
                        self.appropriate_length_id(&i.id)
                            .unwrap_or_else(|_| i.short_id()),
                        i.title,
                        rel.relation_type
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "Cannot delete {} '{}': {} issue(s) reference it:\n{}\nUse --delete-links-too to remove these links and delete anyway.",
                short,
                issue.title,
                referencing.len(),
                listing
            );
        }

        let mut commit_paths: Vec<PathBuf> = vec![issue_path.clone()];
        let mut index = self.storage.load_index()?;
        // Every issue file this operation rewrites or removes is excluded
        // from the stale-index scan: own writes must not look concurrent.
        let mut own_files: Vec<String> = vec![issue.file_name()];

        // Remove the referencing relationships from their source issues.
        for mut source in referencing {
            source.remove_relationship(
                source
                    .relationships
                    .iter()
                    .find(|rel| rel.target_id == uuid)
                    .expect("filtered to referencing issues")
                    .relation_type
                    .clone(),
                uuid,
            );
            let outcome = self.storage.save_issue(&source, &mut index)?;
            commit_paths.push(self.storage.issue_path(&outcome.new_file_name));
            own_files.push(outcome.new_file_name);
        }

        self.storage.delete_issue(&uuid, &index)?;
        index.remove(&uuid);
        let mut warnings = Vec::new();
        let own_refs: Vec<&str> = own_files.iter().map(String::as_str).collect();
        self.storage
            .save_index_stale_aware(&index, &own_refs, &mut warnings)?;
        commit_paths.push(self.storage.index_path().clone());

        // Delete commits the (now-removed) issue file, every file touched by
        // link removal, and index.json.
        let path_refs: Vec<&Path> = commit_paths.iter().map(|p| p.as_path()).collect();
        self.commit_store_fallible(
            &format!("Delete {}: {}", issue.issue_type, issue.title),
            &path_refs,
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: short,
            warnings,
        })
    }

    /// Link `source_id` to `target_id` (`pebbles link`): resolve and load
    /// both issues, reject self-links and any edge that would close a
    /// dependency cycle (r[graph-acyclic]), defer type and duplicate
    /// validation to `Issue::add_relationship`, then save the source issue
    /// and commit the store change.
    pub fn add_relationship(
        &self,
        source_id: &str,
        relation_type: RelationType,
        target_id: &str,
    ) -> Result<MutationOutcome<()>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let source_uuid = self.resolve_id(source_id)?;
        let target_uuid = self.resolve_id(target_id)?;

        if source_uuid == target_uuid {
            // The minimal cycle (r[graph-acyclic]).
            bail!("Cannot link an issue to itself");
        }

        let mut source_issue = self.storage.load_issue(&source_uuid)?;
        let target_issue = self.storage.load_issue(&target_uuid)?;

        // DAG invariant: reject edges that close a directed dependency cycle
        // (r[graph-acyclic]).
        let (all_issues, _) = self.storage.load_all_issues()?;
        let adjacency = build_dependency_adjacency(&all_issues);
        if creates_cycle(&adjacency, source_uuid, target_uuid) {
            bail!(
                "Cannot add relationship {} {} {}: it would create a dependency cycle",
                source_issue.title,
                relation_type,
                target_issue.title
            );
        }

        // Validate relationship
        source_issue
            .add_relationship(relation_type.clone(), target_uuid)
            .map_err(|e| anyhow::anyhow!(e))?;

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&source_issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!(
                "Add relationship: {} {} {}",
                source_issue.title, relation_type, target_issue.title
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: (),
            warnings,
        })
    }

    /// Remove the relationship `source_id --relation-type--> target_id`
    /// (`pebbles unlink`). The target may already be gone (e.g. deleted
    /// while this relationship survived), in which case its short id is
    /// used in messages; fails when the relationship does not exist on the
    /// source.
    pub fn remove_relationship(
        &self,
        source_id: &str,
        relation_type: RelationType,
        target_id: &str,
    ) -> Result<MutationOutcome<()>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let source_uuid = self.resolve_id(source_id)?;
        let target_uuid = self.resolve_id(target_id)?;

        let mut source_issue = self.storage.load_issue(&source_uuid)?;
        // The target may no longer exist (e.g. it was deleted while this
        // relationship survived); fall back to its short id for the message.
        let target_label = self
            .storage
            .load_issue(&target_uuid)
            .map(|issue| issue.title)
            .unwrap_or_else(|_| target_uuid.to_string()[..8].to_string());

        if !source_issue.remove_relationship(relation_type.clone(), target_uuid) {
            bail!("Relationship not found");
        }

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&source_issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!(
                "Remove relationship: {} {} {}",
                source_issue.title, relation_type, target_label
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: (),
            warnings,
        })
    }

    pub fn epic(&self, id: &str) -> Result<(Issue, Vec<Issue>, Vec<LoadWarning>)> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let epic = self.storage.load_issue(&uuid)?;

        if epic.issue_type != IssueType::Epic {
            let short = self.appropriate_length_id(&uuid)?;
            bail!(
                "Issue {} is not an epic (it's a {})",
                short,
                epic.issue_type
            );
        }

        let (all_issues, warnings) = self.storage.load_all_issues()?;
        let mut child_issues: Vec<Issue> = all_issues
            .into_iter()
            .filter(|issue| {
                issue
                    .relationships
                    .iter()
                    .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == uuid)
            })
            .collect();

        // r[impl issue-ordering]
        child_issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        Ok((epic, child_issues, warnings))
    }

    pub fn next(&self) -> Result<(Vec<Issue>, Vec<LoadWarning>)> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let (all_issues, warnings) = self.storage.load_all_issues()?;
        Ok((get_unblocked_issues(&all_issues), warnings))
    }

    /// Returns `Ok(false)` when the issue already has the requested type
    /// (no file write, no jj commit); the caller warns the user.
    pub fn convert(&self, id: &str, new_type: IssueType) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let old_type = issue.issue_type.clone();

        if old_type == new_type {
            return Ok(MutationOutcome {
                value: false,
                warnings: Vec::new(),
            });
        }

        // Validate conversion
        if old_type == IssueType::Epic && new_type != IssueType::Epic {
            // Check if this epic has children
            let (all_issues, _) = self.storage.load_all_issues()?;
            let has_children = all_issues.iter().any(|i| {
                i.relationships
                    .iter()
                    .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == uuid)
            });

            if has_children {
                bail!("Cannot convert epic with child issues to a different type. Remove child relationships first.");
            }
        }

        if new_type == IssueType::Epic && old_type != IssueType::Epic {
            // Check if this issue is part of an epic
            if issue
                .relationships
                .iter()
                .any(|rel| rel.relation_type == RelationType::PartOf)
            {
                bail!("Cannot convert an issue that is part of an epic into an epic itself. Remove part_of relationships first.");
            }
        }

        issue.issue_type = new_type;
        issue.update();

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        let mut warnings = Vec::new();
        self.storage.save_index_stale_aware(
            &index,
            &[outcome.new_file_name.as_str()],
            &mut warnings,
        )?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.commit_store_fallible(
            &format!(
                "Convert {}: {} → {}",
                issue.title, old_type, issue.issue_type
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: true,
            warnings,
        })
    }

    pub fn summary(&self) -> Result<(ProjectSummary, Vec<LoadWarning>)> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let (all_issues, warnings) = self.storage.load_all_issues()?;
        Ok((calculate_summary(&all_issues), warnings))
    }

    /// Epic child progress `(total, done)` computed over the FULL issue set,
    /// independent of any view filter. See SPEC r[epic-progress-counts].
    pub fn epic_counts(&self) -> Result<HashMap<Uuid, (usize, usize)>> {
        let (all_issues, _) = self.storage.load_all_issues()?;
        Ok(build_epic_counts(&all_issues))
    }

    pub fn get_related_titles(&self, issue: &Issue) -> HashMap<Uuid, String> {
        let mut titles = HashMap::new();
        for rel in &issue.relationships {
            if let Ok(related_issue) = self.storage.load_issue(&rel.target_id) {
                titles.insert(rel.target_id, related_issue.title);
            }
        }
        titles
    }

    /// Returns incoming relationships: other issues that have relationships pointing to this issue.
    /// Returns a Vec of (source_issue_id, source_issue_title, relation_type)
    /// plus the load warnings produced while reading the issue set
    /// (r[load-warnings-as-data]).
    pub fn get_incoming_relationships(
        &self,
        issue_id: &Uuid,
    ) -> Result<(Vec<(Uuid, String, RelationType)>, Vec<LoadWarning>)> {
        let (all_issues, warnings) = self.storage.load_all_issues()?;
        let incoming_map = build_incoming_relationships_map(&all_issues);

        let incoming = incoming_map
            .get(issue_id)
            .map(|rels| {
                rels.iter()
                    .map(|r| (r.source_id, r.source_title.clone(), r.relation_type.clone()))
                    .collect()
            })
            .unwrap_or_default();

        Ok((incoming, warnings))
    }

    /// Returns minimal unambiguous ID prefixes for every ID in the index.
    ///
    /// The minimum length is 3 characters. Always returns lowercase.
    /// See SPEC.md for the definition of "appropriate length ID".
    // r[impl appropriate-length-id]
    pub fn id_prefixes(&self) -> Result<HashMap<Uuid, String>> {
        let index = self.storage.load_index()?;
        let all_ids = index.all_ids();
        let id_strs: Vec<(Uuid, String)> = all_ids
            .iter()
            .map(|uuid| (*uuid, uuid.to_string().to_lowercase()))
            .collect();

        let mut prefixes = HashMap::with_capacity(id_strs.len());
        for (uuid, id_str) in &id_strs {
            // Find minimum prefix length (≥3) that uniquely identifies this ID
            let mut prefix = id_str.clone();
            for len in 3..=id_str.len() {
                let candidate = &id_str[..len];
                let matches = id_strs
                    .iter()
                    .filter(|(_, other)| other.starts_with(candidate))
                    .count();
                if matches == 1 {
                    prefix = candidate.to_string();
                    break;
                }
            }
            prefixes.insert(*uuid, prefix);
        }
        Ok(prefixes)
    }

    /// Returns the shortest unambiguous ID prefix for the given UUID.
    /// The minimum length is 3 characters. Always returns lowercase.
    /// See SPEC.md for the definition of "appropriate length ID".
    // r[impl appropriate-length-id]
    pub fn appropriate_length_id(&self, id: &Uuid) -> Result<String> {
        let prefixes = self.id_prefixes()?;
        Ok(prefixes
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string().to_lowercase()))
    }

    // r[impl short-id-input]
    fn resolve_id(&self, id_str: &str) -> Result<Uuid> {
        let index = self.storage.load_index()?;
        match_id_prefix(id_str, &index.all_ids())
    }

    /// Resolve a batch of ID inputs (full UUIDs or unambiguous prefixes)
    /// to their UUIDs, preserving order and dropping duplicates. Fails on
    /// the first unresolvable input, matching per-issue resolution
    /// semantics: a bulk command aborts with the same error the single
    /// command would have produced for that ID.
    pub fn resolve_ids(&self, ids: &[&str]) -> Result<Vec<Uuid>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for id in ids {
            let uuid = self.resolve_id(id)?;
            if seen.insert(uuid) {
                out.push(uuid);
            }
        }
        Ok(out)
    }

    /// Commit the mutation's paths to jj. The store write is already
    /// complete; when jj cannot do its part because there is no repository,
    /// the operation still succeeds and the fact is surfaced as a warning,
    /// so the exit code describes the resulting store state
    /// (r[store-without-jj]).
    fn commit_store_fallible(
        &self,
        message: &str,
        paths: &[&Path],
        warnings: &mut Vec<LoadWarning>,
    ) -> Result<()> {
        match self.jj.commit(message, paths) {
            Ok(()) => Ok(()),
            Err(e) if e.to_string().contains("no jj repo") => {
                warnings.push(LoadWarning {
                    file_name: String::new(),
                    message: format!(
                        "No jj repository found; the change is written to .pebbles/ but was not \
                         committed to version control ({})",
                        e
                    ),
                });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Rebuild `.pebbles/index.json` from the issue files: the sanctioned
    /// repair for a conflicted or desynchronized index (r[reindex]). The
    /// rebuilt index stays in the working copy; the caller commits it when
    /// appropriate (this command deliberately never runs jj - the typical
    /// repair context is a conflicted index after a merge, which jj commit
    /// cannot meaningfully handle).
    pub fn reindex(&self) -> Result<ReindexOutcome> {
        self.storage.reindex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Relationship;

    #[test]
    fn parse_id_input_line_list_row() {
        assert_eq!(
            parse_id_input_line("333 [des] [TODO] JWT token authentication system"),
            vec!["333"]
        );
    }

    #[test]
    fn parse_id_input_line_bare_id() {
        assert_eq!(parse_id_input_line("333"), vec!["333"]);
        assert_eq!(parse_id_input_line("333 "), vec!["333"]);
    }

    #[test]
    fn parse_id_input_line_multiple_hex_tokens() {
        assert_eq!(
            parse_id_input_line("333 444 555"),
            vec!["333", "444", "555"]
        );
    }

    #[test]
    fn parse_id_input_line_full_uuids() {
        let uuid = "33333333-3333-3333-3333-333333333333";
        assert_eq!(parse_id_input_line(uuid), vec![uuid]);
        assert_eq!(
            parse_id_input_line(&format!(
                "{} {}",
                uuid, "11111111-1111-1111-1111-111111111111"
            )),
            vec![uuid, "11111111-1111-1111-1111-111111111111"]
        );
    }

    #[test]
    fn parse_id_input_line_mixed_line_takes_first_field() {
        assert_eq!(parse_id_input_line("333 444 [des] [TODO] JWT"), vec!["333"]);
    }

    #[test]
    fn parse_id_input_line_blank() {
        assert_eq!(parse_id_input_line(""), Vec::<&str>::new());
        assert_eq!(parse_id_input_line("   "), Vec::<&str>::new());
    }

    // -- calculate_summary ----------------------------------------------------

    fn issue_with_type(id: &str, title: &str, status: Status, issue_type: IssueType) -> Issue {
        let mut issue = Issue::new(title.to_string(), None, issue_type);
        issue.id = uuid(id);
        issue.status = status;
        issue
    }

    #[test]
    fn calculate_summary_aggregates_counts_epics_and_actionable() {
        let epic = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Epic",
            Status::Todo,
            IssueType::Epic,
        );
        let mut child1 = issue_with_type(
            "11111111-2222-3333-4444-555555555555",
            "child1",
            Status::Done,
            IssueType::Bug,
        );
        relate(&mut child1, RelationType::PartOf, &epic);
        let mut child2 = issue_with_type(
            "22222222-2222-3333-4444-555555555555",
            "child2",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut child2, RelationType::PartOf, &epic);
        let mut blocker = issue_with_type(
            "33333333-2222-3333-4444-555555555555",
            "blocker",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut blocker, RelationType::RequiredFor, &child2);
        let mut usecase = issue_with_type(
            "44444444-2222-3333-4444-555555555555",
            "usecase",
            Status::InProgress,
            IssueType::Usecase,
        );
        relate(&mut usecase, RelationType::ScheduledBefore, &child2);
        let standalone = issue_with_type(
            "55555555-2222-3333-4444-555555555555",
            "standalone",
            Status::Todo,
            IssueType::Bug,
        );

        let issues = vec![epic.clone(), child1, child2, blocker, usecase, standalone];
        let summary = calculate_summary(&issues);

        assert_eq!(summary.total_issues, 6);

        let expected_by_status: HashMap<Status, usize> = [
            (Status::Todo, 4),
            (Status::Done, 1),
            (Status::InProgress, 1),
        ]
        .into_iter()
        .collect();
        assert_eq!(summary.by_status, expected_by_status);

        let expected_by_type: HashMap<IssueType, usize> = [
            (IssueType::Bug, 4),
            (IssueType::Epic, 1),
            (IssueType::Usecase, 1),
        ]
        .into_iter()
        .collect();
        assert_eq!(summary.by_type, expected_by_type);

        // Epic progress: 2 children, 1 done.
        assert_eq!(summary.epics.len(), 1);
        let (summary_epic, total_children, done_children) = &summary.epics[0];
        assert_eq!(summary_epic.id, epic.id);
        assert_eq!(*total_children, 2);
        assert_eq!(*done_children, 1);

        // Actionable: epic, blocker, standalone (child2 is blocked).
        assert_eq!(summary.actionable_count, 3);

        // Relationships: 2 part_of, 1 required_for, 1 scheduled_before.
        assert_eq!(summary.total_relationships, 4);
        let expected_by_rel: HashMap<RelationType, usize> = [
            (RelationType::PartOf, 2),
            (RelationType::RequiredFor, 1),
            (RelationType::ScheduledBefore, 1),
        ]
        .into_iter()
        .collect();
        assert_eq!(summary.by_relationship_type, expected_by_rel);
    }

    #[test]
    fn calculate_summary_empty_issue_set_is_zeroed() {
        let summary = calculate_summary(&[]);
        assert_eq!(summary.total_issues, 0);
        assert!(summary.by_status.is_empty());
        assert!(summary.by_type.is_empty());
        assert!(summary.epics.is_empty());
        assert_eq!(summary.actionable_count, 0);
        assert_eq!(summary.total_relationships, 0);
        assert!(summary.by_relationship_type.is_empty());
    }

    #[test]
    fn calculate_summary_epic_without_children_reports_zero_progress() {
        let epic = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Lone epic",
            Status::Todo,
            IssueType::Epic,
        );
        let summary = calculate_summary(&[epic]);
        assert_eq!(summary.epics.len(), 1);
        assert_eq!(summary.epics[0].1, 0);
        assert_eq!(summary.epics[0].2, 0);
    }

    // -- get_unblocked_issues -------------------------------------------------

    fn issue_with(id: &str, title: &str, status: Status) -> Issue {
        let mut issue = Issue::new(title.to_string(), None, IssueType::Bug);
        issue.id = uuid(id);
        issue.status = status;
        issue
    }

    /// Record `rel` from `source` pointing at `target` (as `pebbles link
    /// <source> <rel> <target>` does): the relationship lives on the source
    /// issue and the block lands on the target.
    fn relate(source: &mut Issue, rel: RelationType, target: &Issue) {
        source.relationships.push(Relationship {
            relation_type: rel,
            target_id: target.id,
        });
    }

    fn ids(issues: &[Issue]) -> Vec<Uuid> {
        issues.iter().map(|i| i.id).collect()
    }

    #[test]
    fn get_unblocked_issues_includes_only_todo() {
        let a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let proposed = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "P",
            Status::Proposed,
        );
        let done = issue_with("33333333-2222-3333-4444-555555555555", "D", Status::Done);
        let out = get_unblocked_issues(&[a.clone(), proposed, done]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn get_unblocked_issues_required_for_from_non_done_blocks() {
        let mut blocker = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "blocker",
            Status::Todo,
        );
        let blocked = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "blocked",
            Status::Todo,
        );
        relate(&mut blocker, RelationType::RequiredFor, &blocked);
        let out = get_unblocked_issues(&[blocker.clone(), blocked]);
        assert_eq!(ids(&out), vec![blocker.id]);
    }

    #[test]
    fn get_unblocked_issues_scheduled_before_from_non_done_blocks() {
        let mut blocker = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "blocker",
            Status::Todo,
        );
        let blocked = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "blocked",
            Status::Todo,
        );
        relate(&mut blocker, RelationType::ScheduledBefore, &blocked);
        let out = get_unblocked_issues(&[blocker.clone(), blocked]);
        assert_eq!(ids(&out), vec![blocker.id]);
    }

    #[test]
    fn get_unblocked_issues_done_source_does_not_block() {
        let mut done = issue_with("11111111-2222-3333-4444-555555555555", "done", Status::Done);
        let blocked = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "target",
            Status::Todo,
        );
        relate(&mut done, RelationType::RequiredFor, &blocked);
        let out = get_unblocked_issues(&[done, blocked.clone()]);
        assert_eq!(ids(&out), vec![blocked.id]);
    }

    #[test]
    fn get_unblocked_issues_non_done_non_todo_source_blocks() {
        let mut in_progress = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "in-progress",
            Status::InProgress,
        );
        let blocked = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "target",
            Status::Todo,
        );
        relate(&mut in_progress, RelationType::ScheduledBefore, &blocked);
        let out = get_unblocked_issues(&[in_progress, blocked]);
        assert!(out.is_empty());
    }

    #[test]
    fn get_unblocked_issues_part_of_does_not_block() {
        // part_of is a grouping relationship, not a dependency: the target
        // of a part_of edge stays actionable.
        let mut source = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "source",
            Status::Todo,
        );
        let target = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "target",
            Status::Todo,
        );
        relate(&mut source, RelationType::PartOf, &target);
        let mut sorted = vec![source.clone(), target.clone()];
        sorted.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));
        let out = get_unblocked_issues(&[source, target]);
        assert_eq!(ids(&out), ids(&sorted));
    }

    #[test]
    fn get_unblocked_issues_chain_traverses_blocking() {
        // A required_for B, B required_for C: A is actionable, B and C are
        // blocked one hop at a time through incoming non-Done sources.
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        relate(&mut a, RelationType::RequiredFor, &b);
        relate(&mut b, RelationType::RequiredFor, &c);
        let out = get_unblocked_issues(&[a.clone(), b, c]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn get_unblocked_issues_sorted_by_updated_at_desc() {
        let mut older = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "older",
            Status::Todo,
        );
        let mut newer = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "newer",
            Status::Todo,
        );
        older.updated_at = chrono::Utc::now() - chrono::Duration::seconds(60);
        newer.updated_at = chrono::Utc::now();
        let out = get_unblocked_issues(&[older.clone(), newer.clone()]);
        assert_eq!(ids(&out), vec![newer.id, older.id]);
    }

    // -- match_id_prefix ------------------------------------------------------

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn known_ids(ids: &[&str]) -> Vec<Uuid> {
        ids.iter().map(|s| uuid(s)).collect()
    }

    #[test]
    fn match_id_prefix_full_uuid_found() {
        let id1 = uuid("11111111-2222-3333-4444-555555555555");
        let id2 = uuid("aaaaaaaa-1111-2222-3333-444444444444");
        assert_eq!(match_id_prefix(&id1.to_string(), &[id1, id2]).unwrap(), id1);
    }

    #[test]
    fn match_id_prefix_full_uuid_unknown_fails() {
        let known = known_ids(&["11111111-2222-3333-4444-555555555555"]);
        let unknown = uuid("99999999-8888-7777-6666-555555555555");
        let err = match_id_prefix(&unknown.to_string(), &known).unwrap_err();
        assert!(err.to_string().contains("not found"), "{}", err);
    }

    #[test]
    fn match_id_prefix_unique_prefix_resolves() {
        let id1 = uuid("11111111-2222-3333-4444-555555555555");
        let id2 = uuid("aaaaaaaa-1111-2222-3333-444444444444");
        assert_eq!(match_id_prefix("11111111-2222", &[id1, id2]).unwrap(), id1);
    }

    #[test]
    fn match_id_prefix_ambiguous_prefix_fails() {
        let id1 = uuid("11111111-2222-3333-4444-555555555555");
        let id2 = uuid("11111111-9999-8888-7777-666666666666");
        let err = match_id_prefix("11111111", &[id1, id2]).unwrap_err();
        assert!(err.to_string().contains("Ambiguous"), "{}", err);
    }

    #[test]
    fn match_id_prefix_unknown_prefix_fails() {
        let known = known_ids(&["11111111-2222-3333-4444-555555555555"]);
        let err = match_id_prefix("99999999", &known).unwrap_err();
        assert!(err.to_string().contains("No issue found"), "{}", err);
    }

    #[test]
    fn match_id_prefix_case_insensitive_prefix() {
        let id = uuid("abcdef12-3456-7890-abcd-ef1234567890");
        assert_eq!(match_id_prefix("ABCDEF12-3456", &[id]).unwrap(), id);
        assert_eq!(match_id_prefix("abcdef12-3456", &[id]).unwrap(), id);
    }

    #[test]
    fn match_id_prefix_case_insensitive_full_uuid() {
        let id = uuid("abcdef12-3456-7890-abcd-ef1234567890");
        assert_eq!(
            match_id_prefix("ABCDEF12-3456-7890-ABCD-EF1234567890", &[id]).unwrap(),
            id
        );
    }

    #[test]
    fn match_id_prefix_case_boundary_distinguishes_hex_letters() {
        // The ids diverge at a hex letter: 'd' vs 'f'. Uppercase input must
        // normalize to the same lowercase prefix boundary.
        let id_d = uuid("abcdef12-3456-7890-abcd-ef1234567890");
        let id_f = uuid("abcfef12-3456-7890-abcd-ef1234567890");
        assert_eq!(match_id_prefix("ABCD", &[id_d, id_f]).unwrap(), id_d);
        assert_eq!(match_id_prefix("ABCF", &[id_d, id_f]).unwrap(), id_f);
        assert!(match_id_prefix("ABC", &[id_d, id_f]).is_err());
    }

    #[test]
    fn parse_id_input_line_issue_with_hex_title_word() {
        // "Fix" starts with a hex letter but is not all-hex; only the ID
        // field is taken.
        assert_eq!(
            parse_id_input_line("111 [bug] [DONE] Fix mobile login"),
            vec!["111"]
        );
    }

    // -- filter_by_title_query ------------------------------------------------

    #[test]
    fn filter_by_title_query_matches_query_in_any_position() {
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login button",
            Status::Todo,
        );
        let jwt = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "JWT token auth",
            Status::Todo,
        );
        let email = issue_with(
            "33333333-2222-3333-4444-555555555555",
            "User can login with email",
            Status::Todo,
        );
        let out = filter_by_title_query(&[login.clone(), jwt, email.clone()], "login");
        let mut got = ids(&out);
        got.sort();
        let mut want = vec![login.id, email.id];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn filter_by_title_query_is_case_insensitive() {
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        let out = filter_by_title_query(&[login.clone()], "LOGIN");
        assert_eq!(ids(&out), vec![login.id]);
    }

    #[test]
    fn filter_by_title_query_matches_fuzzy_subsequence() {
        // Subsequence matching: 'login' characters appear in order inside
        // 'Login bug' even though the query is not a contiguous substring.
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        let out = filter_by_title_query(&[login.clone()], "lgb");
        assert_eq!(ids(&out), vec![login.id]);
    }

    #[test]
    fn filter_by_title_query_sorts_best_match_first() {
        // The exact contiguous match scores higher than the same letters
        // scattered with gaps, so it sorts first.
        let exact = issue_with("11111111-2222-3333-4444-555555555555", "abc", Status::Todo);
        let scattered = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "a-x-b-x-c",
            Status::Todo,
        );
        let out = filter_by_title_query(&[scattered.clone(), exact.clone()], "abc");
        assert_eq!(ids(&out), vec![exact.id, scattered.id]);
    }

    #[test]
    fn filter_by_title_query_ties_break_by_updated_at_desc() {
        // Equal scores keep the established ordering rule: newer issues
        // first (r[impl issue-ordering]).
        let mut newer = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        let mut older = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        newer.updated_at = chrono::Utc::now();
        older.updated_at = chrono::Utc::now() - chrono::Duration::seconds(60);
        let out = filter_by_title_query(&[older.clone(), newer.clone()], "login");
        assert_eq!(ids(&out), vec![newer.id, older.id]);
    }

    #[test]
    fn filter_by_title_query_no_match_returns_empty() {
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        assert!(filter_by_title_query(&[login.clone()], "zzzz").is_empty());
    }

    #[test]
    fn filter_by_title_query_empty_query_matches_nothing() {
        // A blank --search is a distinct filter (matches nothing), not a
        // silent fallback to the unfiltered list.
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        assert!(filter_by_title_query(&[login.clone()], "").is_empty());
        assert!(filter_by_title_query(&[login.clone()], "   ").is_empty());
    }
}
