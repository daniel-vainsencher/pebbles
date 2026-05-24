use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::jj::JJ;
use crate::models::{build_incoming_relationships_map, Issue, IssueType, RelationType, Status};
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

pub struct Operations {
    storage: Storage,
    jj: JJ,
}

/// Build the list of paths to pass to `JJ::commit` based on what
/// `Storage::save_issue` did. Always includes the new issue file;
/// also includes the old issue file (deletion) on rename, and
/// `.pebbles/index.json` when the index was modified.
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

    pub fn create(
        &self,
        title: String,
        description: Option<String>,
        issue_type: IssueType,
    ) -> Result<Issue> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let issue = Issue::new(title, description, issue_type);

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

        issues.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(issues)
    }

    pub fn show(&self, id: &str) -> Result<Issue> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        self.storage.load_issue(&uuid)
    }

    pub fn update_status(&self, id: &str, new_status: Status) -> Result<()> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

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

        Ok(())
    }

    pub fn update(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
    ) -> Result<()> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        if let Some(new_title) = title {
            issue.title = new_title;
        }

        if let Some(new_desc) = description {
            issue.description = Some(new_desc);
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

        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let issue = self.storage.load_issue(&uuid)?;
        let issue_path = self.storage.issue_path(&issue.file_name());

        let mut index = self.storage.load_index()?;
        self.storage.delete_issue(&uuid, &index)?;
        index.remove(&uuid);
        self.storage.save_index(&index)?;

        // Delete commits the (now-removed) issue file *and* index.json,
        // which had its entry removed.
        let index_path = self.storage.index_path().clone();
        self.jj.commit(
            &format!("Delete {}: {}", issue.issue_type, issue.title),
            &[issue_path.as_path(), index_path.as_path()],
        )?;

        Ok(())
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

        let mut source_issue = self.storage.load_issue(&source_uuid)?;
        let target_issue = self.storage.load_issue(&target_uuid)?;

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

        if !source_issue.remove_relationship(relation_type.clone(), target_uuid) {
            bail!("Relationship not found");
        }

        let mut index = self.storage.load_index()?;
        let outcome = self.storage.save_issue(&source_issue, &mut index)?;
        self.storage.save_index(&index)?;

        let paths = paths_for_save(&self.storage, &outcome);
        self.jj.commit(
            &format!(
                "Remove relationship: {} {} <target>",
                source_issue.title, relation_type
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
            bail!("Issue {} is not an epic (it's a {})", uuid, epic.issue_type);
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

        child_issues.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

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

        unblocked_issues.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(unblocked_issues)
    }

    pub fn convert(&self, id: &str, new_type: IssueType) -> Result<()> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let old_type = issue.issue_type.clone();

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

        Ok(())
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

    /// Returns the shortest unambiguous ID prefix for the given UUID.
    /// The minimum length is 3 characters. Always returns lowercase.
    /// See SPEC.md for the definition of "appropriate length ID".
    pub fn appropriate_length_id(&self, id: &Uuid) -> Result<String> {
        let index = self.storage.load_index()?;
        let all_ids = index.all_ids();
        let id_str = id.to_string().to_lowercase();

        // Find minimum prefix length (≥3) that uniquely identifies this ID
        for len in 3..=id_str.len() {
            let prefix = &id_str[..len];
            let matches = all_ids
                .iter()
                .filter(|uuid| uuid.to_string().to_lowercase().starts_with(prefix))
                .count();
            if matches == 1 {
                return Ok(prefix.to_string());
            }
        }

        // Fallback to full ID if somehow not unique (shouldn't happen)
        Ok(id_str)
    }

    fn resolve_id(&self, id_str: &str) -> Result<Uuid> {
        let index = self.storage.load_index()?;
        let all_ids = index.all_ids();

        if let Ok(uuid) = Uuid::parse_str(id_str) {
            if all_ids.contains(&uuid) {
                return Ok(uuid);
            }
            bail!("Issue with ID {} not found", uuid);
        }

        let matches: Vec<_> = all_ids
            .iter()
            .filter(|uuid| uuid.to_string().starts_with(id_str))
            .collect();

        match matches.len() {
            0 => bail!("No issue found with ID starting with '{}'", id_str),
            1 => Ok(*matches[0]),
            _ => bail!("Ambiguous ID '{}'. Multiple issues match.", id_str),
        }
    }
}
