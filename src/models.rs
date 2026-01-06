use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Todo,
    InProgress,
    Done,
    Blocked,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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
            "todo" => Ok(Status::Todo),
            "in_progress" | "inprogress" | "in-progress" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            "blocked" => Ok(Status::Blocked),
            _ => Err(format!("Invalid status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
            "required_for" | "requiredfor" | "required-for" | "requires" => Ok(RelationType::RequiredFor),
            "scheduled_before" | "scheduledbefore" | "scheduled-before" | "before" => Ok(RelationType::ScheduledBefore),
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
    pub fn new(title: String, description: Option<String>, issue_type: IssueType) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            title,
            description,
            status: Status::Todo,
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

    pub fn file_name(&self) -> String {
        let slug = slug::slugify(&self.title);
        let short_slug = if slug.len() > 40 {
            &slug[..40]
        } else {
            &slug
        };
        format!("{}-{}-{}.json", self.issue_type.short_code(), self.short_id(), short_slug)
    }

    pub fn add_relationship(&mut self, relation_type: RelationType, target_id: Uuid) -> Result<(), String> {
        // Validate relationship based on issue types
        if relation_type == RelationType::PartOf {
            if !self.issue_type.can_be_part_of_epic() {
                return Err(format!("{} cannot be part of an epic", self.issue_type));
            }
        }

        // Check for duplicates
        if self.relationships.iter().any(|r| r.relation_type == relation_type && r.target_id == target_id) {
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
        self.relationships.retain(|r| !(r.relation_type == relation_type && r.target_id == target_id));
        let removed = self.relationships.len() < original_len;
        if removed {
            self.update();
        }
        removed
    }

    pub fn get_relationships(&self, relation_type: RelationType) -> Vec<Uuid> {
        self.relationships
            .iter()
            .filter(|r| r.relation_type == relation_type)
            .map(|r| r.target_id)
            .collect()
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
        Self {
            issues: Vec::new(),
        }
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
        self.issues.iter()
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
