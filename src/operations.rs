use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::jj::JJ;
use crate::models::{
    build_dependency_adjacency, build_epic_counts, build_incoming_relationships_map, creates_cycle,
    Issue, IssueType, RelationType, Status,
};
use crate::storage::{SaveOutcome, Storage};

pub struct ProjectSummary {
    pub total_issues: usize,
    pub by_status: HashMap<Status, usize>,
    pub by_type: HashMap<IssueType, usize>,
    pub epics: Vec<(Issue, usize, usize)>, // (epic, total_children, done_children)
    pub actionable_count: usize,
    pub total_relationships: usize,
    pub by_relationship_type: HashMap<RelationType, usize>,
}

/// Titles must be non-empty after trimming: they become slugs in filenames
/// and the primary human-readable handle for an issue.
fn validate_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        bail!("Issue title cannot be empty");
    }
    Ok(())
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

impl Operations {
    pub fn new(repo_path: &Path) -> Result<Self> {
        let storage = Storage::new(repo_path)?;
        let jj = JJ::new(repo_path)?;

        Ok(Self { storage, jj })
    }

    pub fn init(&self) -> Result<()> {
        if self.storage.exists() {
            bail!("Pebbles is already initialized in this repository");
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
    ) -> Result<Issue> {
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
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!("Create {}: {}", issue.issue_type, issue.title),
            &as_path_refs(&paths),
        )?;

        Ok(issue)
    }

    // r[impl list-filter]
    pub fn list(
        &self,
        status_filter: Option<Status>,
        type_filter: Option<IssueType>,
    ) -> Result<Vec<Issue>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let mut issues = self.storage.load_all_issues()?;

        if let Some(status) = status_filter {
            issues.retain(|issue| issue.status == status);
        }

        if let Some(issue_type) = type_filter {
            issues.retain(|issue| issue.issue_type == issue_type);
        }

        // r[impl issue-ordering]
        issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        Ok(issues)
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
    pub fn update_status(&self, id: &str, new_status: Status) -> Result<bool> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        if issue.status == new_status {
            return Ok(false);
        }

        let old_status = issue.status.clone();
        issue.status = new_status;
        issue.update();

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!(
                "Update {} {}: {} -> {}",
                issue.issue_type, issue.title, old_status, issue.status
            ),
            &as_path_refs(&paths),
        )?;

        Ok(true)
    }

    // r[impl approve-command]
    pub fn approve(&self, id: &str) -> Result<()> {
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
        self.update_status(id, Status::Todo)?;
        Ok(())
    }

    /// Returns `Ok(false)` when nothing changed (no file write, no jj commit).
    pub fn update(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
    ) -> Result<bool> {
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
            return Ok(false);
        }

        issue.update();

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&issue, &mut index)?;
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!("Update {}: {}", issue.issue_type, issue.title),
            &as_path_refs(&paths),
        )?;

        Ok(true)
    }

    /// Deletes the issue and returns its appropriate-length ID (computed
    /// while the index still contains it) for display purposes.
    ///
    /// r[impl no-broken-links]: if other issues reference this one, the
    /// delete fails naming each broken link unless `delete_links_too` is
    /// set, in which case the referencing relationships are removed in the
    /// same operation and commit.
    pub fn delete(&self, id: &str, delete_links_too: bool) -> Result<String> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let short = self.appropriate_length_id(&uuid)?;
        let issue = self.storage.load_issue(&uuid)?;
        let issue_path = self.storage.issue_path(&issue.file_name());

        // Find every issue with a relationship pointing at this one.
        let all_issues = self.storage.load_all_issues()?;
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
        }

        self.storage.delete_issue(&uuid, &index)?;
        index.remove(&uuid);
        self.storage.save_index(&index)?;
        commit_paths.push(self.storage.index_path().clone());

        // Delete commits the (now-removed) issue file, every file touched by
        // link removal, and index.json.
        let path_refs: Vec<&Path> = commit_paths.iter().map(|p| p.as_path()).collect();
        self.jj.commit(
            &format!("Delete {}: {}", issue.issue_type, issue.title),
            &path_refs,
        )?;

        Ok(short)
    }

    pub fn add_relationship(
        &self,
        source_id: &str,
        relation_type: RelationType,
        target_id: &str,
    ) -> Result<()> {
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
        let all_issues = self.storage.load_all_issues()?;
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
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!(
                "Add relationship: {} {} {}",
                source_issue.title, relation_type, target_issue.title
            ),
            &as_path_refs(&paths),
        )?;

        Ok(())
    }

    pub fn remove_relationship(
        &self,
        source_id: &str,
        relation_type: RelationType,
        target_id: &str,
    ) -> Result<()> {
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
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!(
                "Remove relationship: {} {} {}",
                source_issue.title, relation_type, target_label
            ),
            &as_path_refs(&paths),
        )?;

        Ok(())
    }

    pub fn epic(&self, id: &str) -> Result<(Issue, Vec<Issue>)> {
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

        let all_issues = self.storage.load_all_issues()?;
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

        Ok((epic, child_issues))
    }

    pub fn next(&self) -> Result<Vec<Issue>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let all_issues = self.storage.load_all_issues()?;
        let incoming_map = build_incoming_relationships_map(&all_issues);

        let todo_issues: Vec<Issue> = all_issues
            .iter()
            .filter(|issue| issue.status == Status::Todo)
            .cloned()
            .collect();

        // An issue is blocked if it has an incoming RequiredFor or ScheduledBefore
        // from an issue that is not Done.
        let mut unblocked_issues: Vec<Issue> = todo_issues
            .into_iter()
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
            .collect();

        // r[impl issue-ordering]
        unblocked_issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        Ok(unblocked_issues)
    }

    /// Returns `Ok(false)` when the issue already has the requested type
    /// (no file write, no jj commit); the caller warns the user.
    pub fn convert(&self, id: &str, new_type: IssueType) -> Result<bool> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let old_type = issue.issue_type.clone();

        if old_type == new_type {
            return Ok(false);
        }

        // Validate conversion
        if old_type == IssueType::Epic && new_type != IssueType::Epic {
            // Check if this epic has children
            let all_issues = self.storage.load_all_issues()?;
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
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!(
                "Convert {}: {} → {}",
                issue.title, old_type, issue.issue_type
            ),
            &as_path_refs(&paths),
        )?;

        Ok(true)
    }

    pub fn summary(&self) -> Result<ProjectSummary> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let all_issues = self.storage.load_all_issues()?;
        let total_issues = all_issues.len();

        // Count by status
        let mut by_status: HashMap<Status, usize> = HashMap::new();
        for issue in &all_issues {
            *by_status.entry(issue.status.clone()).or_insert(0) += 1;
        }

        // Count by type
        let mut by_type: HashMap<IssueType, usize> = HashMap::new();
        for issue in &all_issues {
            *by_type.entry(issue.issue_type.clone()).or_insert(0) += 1;
        }

        // Gather epic info
        let epics: Vec<Issue> = all_issues
            .iter()
            .filter(|i| i.issue_type == IssueType::Epic)
            .cloned()
            .collect();

        let mut epic_info = Vec::new();
        for epic in epics {
            let children: Vec<&Issue> = all_issues
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
        let actionable = self.next()?;
        let actionable_count = actionable.len();

        // Count relationships
        let mut total_relationships = 0;
        let mut by_relationship_type: HashMap<RelationType, usize> = HashMap::new();
        for issue in &all_issues {
            for rel in &issue.relationships {
                total_relationships += 1;
                *by_relationship_type
                    .entry(rel.relation_type.clone())
                    .or_insert(0) += 1;
            }
        }

        Ok(ProjectSummary {
            total_issues,
            by_status,
            by_type,
            epics: epic_info,
            actionable_count,
            total_relationships,
            by_relationship_type,
        })
    }

    /// Epic child progress `(total, done)` computed over the FULL issue set,
    /// independent of any view filter. See SPEC r[epic-progress-counts].
    pub fn epic_counts(&self) -> Result<HashMap<Uuid, (usize, usize)>> {
        let all_issues = self.storage.load_all_issues()?;
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
    pub fn get_incoming_relationships(
        &self,
        issue_id: &Uuid,
    ) -> Result<Vec<(Uuid, String, RelationType)>> {
        let all_issues = self.storage.load_all_issues()?;
        let incoming_map = build_incoming_relationships_map(&all_issues);

        let incoming = incoming_map
            .get(issue_id)
            .map(|rels| {
                rels.iter()
                    .map(|r| (r.source_id, r.source_title.clone(), r.relation_type.clone()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(incoming)
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
        let all_ids = index.all_ids();

        if let Ok(uuid) = Uuid::parse_str(id_str) {
            if all_ids.contains(&uuid) {
                return Ok(uuid);
            }
            bail!("Issue with ID {} not found", uuid);
        }

        // Prefix matching is case-insensitive (SPEC r[short-id-input]).
        let id_str = id_str.to_lowercase();
        let matches: Vec<_> = all_ids
            .iter()
            .filter(|uuid| uuid.to_string().starts_with(&id_str))
            .collect();

        match matches.len() {
            0 => bail!("No issue found with ID starting with '{}'", id_str),
            1 => Ok(*matches[0]),
            _ => bail!("Ambiguous ID '{}'. Multiple issues match.", id_str),
        }
    }
}
