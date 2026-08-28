use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// r[impl status-serialization]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Proposed,
    Todo,
    // Canonical on-disk spelling is `in_progress` (matches Display, FromStr,
    // and the CLI docs). Legacy files written as `inprogress` still load.
    #[serde(rename = "in_progress", alias = "inprogress")]
    InProgress,
    Done,
    Blocked,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Proposed => write!(f, "proposed"),
            Status::Todo => write!(f, "todo"),
            Status::InProgress => write!(f, "in_progress"),
            Status::Done => write!(f, "done"),
            Status::Blocked => write!(f, "blocked"),
        }
    }
}

impl std::str::FromStr for Status {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "proposed" => Ok(Status::Proposed),
            "todo" => Ok(Status::Todo),
            "in_progress" | "inprogress" | "in-progress" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            "blocked" => Ok(Status::Blocked),
            _ => Err(format!("Invalid status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    Bug,
    UxProblem,
    Usecase,
    Design,
    Validation,
    Epic,
    Milestone,
}

impl std::fmt::Display for IssueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueType::Bug => write!(f, "bug"),
            IssueType::UxProblem => write!(f, "ux_problem"),
            IssueType::Usecase => write!(f, "usecase"),
            IssueType::Design => write!(f, "design"),
            IssueType::Validation => write!(f, "validation"),
            IssueType::Epic => write!(f, "epic"),
            IssueType::Milestone => write!(f, "milestone"),
        }
    }
}

impl std::str::FromStr for IssueType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "bug" => Ok(IssueType::Bug),
            "ux_problem" | "uxproblem" | "ux-problem" | "ux" => Ok(IssueType::UxProblem),
            "usecase" | "use-case" | "use_case" => Ok(IssueType::Usecase),
            "design" => Ok(IssueType::Design),
            "validation" => Ok(IssueType::Validation),
            "epic" => Ok(IssueType::Epic),
            "milestone" => Ok(IssueType::Milestone),
            _ => Err(format!("Invalid issue type: {}", s)),
        }
    }
}

impl IssueType {
    // r[impl part-of-epic-types]
    pub fn can_be_part_of_epic(&self) -> bool {
        !matches!(self, IssueType::Epic | IssueType::Milestone)
    }

    pub fn short_code(&self) -> &'static str {
        match self {
            IssueType::Bug => "bug",
            IssueType::UxProblem => "ux",
            IssueType::Usecase => "uc",
            IssueType::Design => "des",
            IssueType::Validation => "val",
            IssueType::Epic => "epic",
            IssueType::Milestone => "mile",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    PartOf,
    RequiredFor,
    ScheduledBefore,
}

impl std::fmt::Display for RelationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationType::PartOf => write!(f, "part_of"),
            RelationType::RequiredFor => write!(f, "required_for"),
            RelationType::ScheduledBefore => write!(f, "scheduled_before"),
        }
    }
}

impl std::str::FromStr for RelationType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "part_of" | "partof" | "part-of" => Ok(RelationType::PartOf),
            "required_for" | "requiredfor" | "required-for" | "requires" => {
                Ok(RelationType::RequiredFor)
            }
            "scheduled_before" | "scheduledbefore" | "scheduled-before" | "before" => {
                Ok(RelationType::ScheduledBefore)
            }
            _ => Err(format!("Invalid relation type: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub relation_type: RelationType,
    pub target_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub issue_type: IssueType,
    pub tags: Vec<String>,
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Issue {
    // r[impl new-issues-default-proposed]
    pub fn new(title: String, description: Option<String>, issue_type: IssueType) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title,
            description,
            status: Status::Proposed,
            issue_type,
            tags: Vec::new(),
            relationships: Vec::new(),
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub fn update(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn short_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    // r[impl issue-filename-format]
    pub fn file_name(&self) -> String {
        let slug = slug::slugify(&self.title);
        let short_slug = if slug.len() > 40 { &slug[..40] } else { &slug };
        format!(
            "{}-{}-{}.json",
            self.issue_type.short_code(),
            self.short_id(),
            short_slug
        )
    }

    // r[impl part-of-epic-types]
    // r[impl no-duplicate-relationships]
    pub fn add_relationship(
        &mut self,
        relation_type: RelationType,
        target_id: Uuid,
    ) -> Result<(), String> {
        // Validate relationship based on issue types
        if relation_type == RelationType::PartOf && !self.issue_type.can_be_part_of_epic() {
            return Err(format!("{} cannot be part of an epic", self.issue_type));
        }

        // Check for duplicates
        if self
            .relationships
            .iter()
            .any(|r| r.relation_type == relation_type && r.target_id == target_id)
        {
            return Err("Relationship already exists".to_string());
        }

        self.relationships.push(Relationship {
            relation_type,
            target_id,
        });
        self.update();
        Ok(())
    }

    pub fn remove_relationship(&mut self, relation_type: RelationType, target_id: Uuid) -> bool {
        let original_len = self.relationships.len();
        self.relationships
            .retain(|r| !(r.relation_type == relation_type && r.target_id == target_id));
        let removed = self.relationships.len() < original_len;
        if removed {
            self.update();
        }
        removed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueMetadata {
    pub id: Uuid,
    pub file_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub issues: Vec<IssueMetadata>,
}

impl Index {
    pub fn new() -> Self {
        Self { issues: Vec::new() }
    }

    pub fn add(&mut self, id: Uuid, file_name: String) {
        self.issues.push(IssueMetadata { id, file_name });
    }

    pub fn remove(&mut self, id: &Uuid) -> Option<String> {
        if let Some(pos) = self.issues.iter().position(|m| &m.id == id) {
            Some(self.issues.remove(pos).file_name)
        } else {
            None
        }
    }

    pub fn get_file_name(&self, id: &Uuid) -> Option<&str> {
        self.issues
            .iter()
            .find(|m| &m.id == id)
            .map(|m| m.file_name.as_str())
    }

    pub fn update_file_name(&mut self, id: &Uuid, new_file_name: String) -> bool {
        if let Some(metadata) = self.issues.iter_mut().find(|m| &m.id == id) {
            metadata.file_name = new_file_name;
            true
        } else {
            false
        }
    }

    pub fn all_ids(&self) -> Vec<Uuid> {
        self.issues.iter().map(|m| m.id).collect()
    }
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about an incoming relationship: source issue details and relation type.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingRelationship {
    pub source_id: Uuid,
    pub source_title: String,
    pub source_status: Status,
    pub relation_type: RelationType,
}

/// Builds a map of incoming relationships from a list of issues.
/// Key: target issue ID, Value: list of incoming relationships pointing to that issue.
/// This is a pure function that can be tested without I/O.
pub fn build_incoming_relationships_map(
    issues: &[Issue],
) -> HashMap<Uuid, Vec<IncomingRelationship>> {
    let mut map: HashMap<Uuid, Vec<IncomingRelationship>> = HashMap::new();
    for issue in issues {
        for rel in &issue.relationships {
            map.entry(rel.target_id)
                .or_default()
                .push(IncomingRelationship {
                    source_id: issue.id,
                    source_title: issue.title.clone(),
                    source_status: issue.status.clone(),
                    relation_type: rel.relation_type.clone(),
                });
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_issue(title: &str, status: Status) -> Issue {
        let mut issue = Issue::new(title.to_string(), None, IssueType::Bug);
        issue.status = status;
        issue
    }

    #[test]
    fn test_status_proposed_roundtrip() {
        // r[verify new-issues-default-proposed]
        assert_eq!(Status::Proposed.to_string(), "proposed");
        assert_eq!("proposed".parse::<Status>(), Ok(Status::Proposed));
        assert_eq!("PROPOSED".parse::<Status>(), Ok(Status::Proposed));
        assert_eq!(
            Status::Proposed,
            serde_json::from_str("\"proposed\"").unwrap()
        );
        assert_eq!(
            serde_json::to_string(&Status::Proposed).unwrap(),
            "\"proposed\""
        );

        // A newly constructed issue starts as proposed.
        let issue = Issue::new("Fresh".to_string(), None, IssueType::Bug);
        assert_eq!(issue.status, Status::Proposed);
    }

    #[test]
    fn test_status_in_progress_serialization() {
        // r[verify status-serialization]
        // On disk the status is written with its canonical spelling.
        assert_eq!(
            serde_json::to_string(&Status::InProgress).unwrap(),
            "\"in_progress\""
        );

        // The canonical spelling parses...
        assert_eq!(
            serde_json::from_str::<Status>("\"in_progress\"").unwrap(),
            Status::InProgress
        );
        assert_eq!("in_progress".parse::<Status>(), Ok(Status::InProgress));

        // ...and so does the legacy `inprogress` form written by older
        // versions of pebbles, so existing data keeps loading.
        assert_eq!(
            serde_json::from_str::<Status>("\"inprogress\"").unwrap(),
            Status::InProgress
        );
        assert_eq!("inprogress".parse::<Status>(), Ok(Status::InProgress));
    }

    #[test]
    fn test_build_incoming_relationships_map_empty() {
        let issues: Vec<Issue> = vec![];
        let map = build_incoming_relationships_map(&issues);
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_incoming_relationships_map_no_relationships() {
        let issues = vec![make_issue("A", Status::Todo), make_issue("B", Status::Todo)];
        let map = build_incoming_relationships_map(&issues);
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_incoming_relationships_map_single_relationship() {
        let mut a = make_issue("A", Status::Todo);
        let b = make_issue("B", Status::Todo);
        a.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });

        let map = build_incoming_relationships_map(&[a.clone(), b.clone()]);

        // A has no incoming relationships
        assert!(!map.contains_key(&a.id));

        // B has one incoming relationship from A
        let b_incoming = map.get(&b.id).unwrap();
        assert_eq!(b_incoming.len(), 1);
        assert_eq!(b_incoming[0].source_id, a.id);
        assert_eq!(b_incoming[0].source_title, "A");
        assert_eq!(b_incoming[0].source_status, Status::Todo);
        assert_eq!(b_incoming[0].relation_type, RelationType::RequiredFor);
    }

    #[test]
    fn test_build_incoming_relationships_map_multiple_relationships() {
        let mut a = make_issue("A", Status::Done);
        let mut b = make_issue("B", Status::Todo);
        let c = make_issue("C", Status::Todo);

        // A required_for C
        a.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: c.id,
        });
        // B scheduled_before C
        b.relationships.push(Relationship {
            relation_type: RelationType::ScheduledBefore,
            target_id: c.id,
        });

        let map = build_incoming_relationships_map(&[a.clone(), b.clone(), c.clone()]);

        // C has two incoming relationships
        let c_incoming = map.get(&c.id).unwrap();
        assert_eq!(c_incoming.len(), 2);

        // Verify both relationships are present (order may vary)
        let from_a = c_incoming.iter().find(|r| r.source_id == a.id).unwrap();
        assert_eq!(from_a.source_status, Status::Done);
        assert_eq!(from_a.relation_type, RelationType::RequiredFor);

        let from_b = c_incoming.iter().find(|r| r.source_id == b.id).unwrap();
        assert_eq!(from_b.source_status, Status::Todo);
        assert_eq!(from_b.relation_type, RelationType::ScheduledBefore);
    }
}
