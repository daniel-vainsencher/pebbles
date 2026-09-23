use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

// r[impl status-serialization]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
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

    /// Parse a status from user or file input, case-insensitively.
    /// `in_progress` matches the canonical on-disk spelling; the legacy
    /// `inprogress` and `in-progress` spellings also parse
    /// (r[impl status-serialization]).
    ///
    /// ```rust
    /// # use pebbles::models::Status;
    /// assert_eq!("todo".parse::<Status>(), Ok(Status::Todo));
    /// assert_eq!("IN-PROGRESS".parse::<Status>(), Ok(Status::InProgress));
    /// assert!("what".parse::<Status>().is_err());
    /// ```
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
    // Canonical on-disk spelling is `ux_problem` (matches Display, FromStr,
    // and the CLI docs). Legacy files written with the CLI shorthand `ux`
    // still load.
    #[serde(alias = "ux")]
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

    /// Parse an issue type from user or file input, case-insensitively.
    /// `usecase` also accepts `use-case` and `use_case`; `ux_problem` also
    /// accepts the legacy `uxproblem` and `ux-problem` spellings and the
    /// CLI shorthand `ux`.
    ///
    /// ```rust
    /// # use pebbles::models::IssueType;
    /// assert_eq!("use-case".parse::<IssueType>(), Ok(IssueType::Usecase));
    /// assert_eq!("use_case".parse::<IssueType>(), Ok(IssueType::Usecase));
    /// assert_eq!("UX".parse::<IssueType>(), Ok(IssueType::UxProblem));
    /// assert!("feature".parse::<IssueType>().is_err());
    /// ```
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
    /// Whether an issue of this type may be linked into an epic with
    /// `part_of`: every type except `Epic` and `Milestone`
    /// (r[impl part-of-epic-types]).
    ///
    /// ```rust
    /// # use pebbles::models::IssueType;
    /// assert!(IssueType::Bug.can_be_part_of_epic());
    /// assert!(!IssueType::Epic.can_be_part_of_epic());
    /// assert!(!IssueType::Milestone.can_be_part_of_epic());
    /// ```
    // r[impl part-of-epic-types]
    pub fn can_be_part_of_epic(&self) -> bool {
        !matches!(self, IssueType::Epic | IssueType::Milestone)
    }

    /// The 2-4 character prefix used in issue file names
    /// (r[impl issue-filename-format]): `bug`, `ux`, `uc`, `des`, `val`,
    /// `epic`, `mile`.
    ///
    /// ```rust
    /// # use pebbles::models::IssueType;
    /// assert_eq!(IssueType::UxProblem.short_code(), "ux");
    /// assert_eq!(IssueType::Usecase.short_code(), "uc");
    /// assert_eq!(IssueType::Epic.short_code(), "epic");
    /// assert_eq!(IssueType::Milestone.short_code(), "mile");
    /// ```
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

    /// Parse a relation type from user or file input, case-insensitively.
    /// Each type accepts its snake_case spelling plus the joined form;
    /// `required_for` also accepts `requires` and `scheduled_before` also
    /// accepts `before`.
    ///
    /// ```rust
    /// # use pebbles::models::RelationType;
    /// assert_eq!("part-of".parse::<RelationType>(), Ok(RelationType::PartOf));
    /// assert_eq!("requires".parse::<RelationType>(), Ok(RelationType::RequiredFor));
    /// assert_eq!("BEFORE".parse::<RelationType>(), Ok(RelationType::ScheduledBefore));
    /// assert!("blocks".parse::<RelationType>().is_err());
    /// ```
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
    /// Create an issue with a fresh UUID, status `Proposed`, empty tags,
    /// relationships, and metadata, and both timestamps set to the current
    /// time. `--approved` callers flip the status to `Todo` after creation
    /// (r[impl new-issues-default-proposed]).
    ///
    /// ```rust
    /// # use pebbles::models::{Issue, IssueType, Status};
    /// let issue = Issue::new("Title".to_string(), None, IssueType::Bug);
    /// assert_eq!(issue.status, Status::Proposed);
    /// assert!(issue.tags.is_empty());
    /// assert!(issue.relationships.is_empty());
    /// ```
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

    /// Bump `updated_at` to the current time. Mutations call this before
    /// saving so the record reflects when it last changed.
    pub fn update(&mut self) {
        self.updated_at = Utc::now();
    }

    /// The 8-character ID used in issue filenames.
    /// Stable regardless of other issues in the repo (SPEC r[issue-filename-format]).
    /// User-facing display uses the minimal unambiguous prefix instead
    /// (SPEC r[appropriate-length-id]); see `Operations::id_prefixes`.
    ///
    /// ```rust
    /// # use pebbles::models::{Issue, IssueType};
    /// let issue = Issue::new("Title".to_string(), None, IssueType::Bug);
    /// let short = issue.short_id();
    /// assert_eq!(short.len(), 8);
    /// assert!(issue.id.to_string().starts_with(&short));
    /// ```
    pub fn short_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    /// The on-disk file name for this issue:
    /// `{type-short-code}-{short_id}-{slug}.json`, where the slug is the
    /// URL-slugified title truncated to 40 characters
    /// (r[impl issue-filename-format]).
    ///
    /// ```rust
    /// # use pebbles::models::{Issue, IssueType};
    /// # use uuid::Uuid;
    /// let mut issue = Issue::new("My cool feature".to_string(), None, IssueType::Usecase);
    /// issue.id = Uuid::parse_str("12345678-90ab-cdef-1234-567890abcdef").unwrap();
    /// assert_eq!(issue.file_name(), "uc-12345678-my-cool-feature.json");
    ///
    /// // Titles that slug to more than 40 characters are truncated.
    /// let mut long = Issue::new("x".repeat(50).to_string(), None, IssueType::Bug);
    /// long.id = Uuid::parse_str("aaaaaaaa-90ab-cdef-1234-567890abcdef").unwrap();
    /// let expected = format!("bug-aaaaaaaa-{}.json", "x".repeat(40));
    /// assert_eq!(long.file_name(), expected);
    /// ```
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

    /// Add a directed relationship to `target_id`, rejecting:
    ///
    /// - `part_of` from an `Epic` or `Milestone` source
    ///   (r[impl part-of-epic-types]);
    /// - an exact duplicate of an existing relationship
    ///   (r[impl no-duplicate-relationships]).
    ///
    /// On success the relationship is appended and `updated_at` bumped.
    ///
    /// ```rust
    /// # use pebbles::models::{Issue, IssueType, RelationType};
    /// let mut epic = Issue::new("Epic".to_string(), None, IssueType::Epic);
    /// let mut issue = Issue::new("Child".to_string(), None, IssueType::Bug);
    ///
    /// // An epic cannot be part of another epic.
    /// assert!(epic.add_relationship(RelationType::PartOf, issue.id).is_err());
    ///
    /// // A bug can; the relationship lives on the source issue.
    /// assert!(issue.add_relationship(RelationType::PartOf, epic.id).is_ok());
    /// assert_eq!(issue.relationships.len(), 1);
    ///
    /// // Duplicates are rejected.
    /// assert!(issue.add_relationship(RelationType::PartOf, epic.id).is_err());
    /// assert_eq!(issue.relationships.len(), 1);
    /// ```
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

    /// Remove every relationship of `relation_type` pointing at
    /// `target_id`. Returns whether anything was removed; `updated_at` is
    /// bumped only on an actual removal.
    ///
    /// ```rust
    /// # use pebbles::models::{Issue, IssueType, RelationType};
    /// let epic = Issue::new("Epic".to_string(), None, IssueType::Epic);
    /// let mut issue = Issue::new("Child".to_string(), None, IssueType::Bug);
    /// issue.add_relationship(RelationType::PartOf, epic.id).unwrap();
    /// assert!(issue.remove_relationship(RelationType::PartOf, epic.id));
    /// assert!(issue.relationships.is_empty());
    /// assert!(!issue.remove_relationship(RelationType::PartOf, epic.id));
    /// ```
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
    /// An empty index. The initial state a store loads when `index.json`
    /// is missing.
    pub fn new() -> Self {
        Self { issues: Vec::new() }
    }

    /// Record that `id` maps to `file_name` in the index. Callers keep the
    /// mapping in sync with the issue files on disk (`Storage::save_issue`
    /// and `plan_save_issue` do this).
    pub fn add(&mut self, id: Uuid, file_name: String) {
        self.issues.push(IssueMetadata { id, file_name });
    }

    /// Remove the entry for `id`, returning the file name it mapped to,
    /// or `None` when the id is not indexed.
    ///
    /// ```rust
    /// # use pebbles::models::Index;
    /// # use uuid::Uuid;
    /// let id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    /// let mut index = Index::new();
    /// index.add(id, "bug-11111111-x.json".to_string());
    /// assert_eq!(index.remove(&id), Some("bug-11111111-x.json".to_string()));
    /// assert_eq!(index.remove(&id), None);
    /// ```
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

/// Count each epic's `part_of` children as `(total, done)` by epic ID.
/// Pure function, testable without I/O.
pub fn build_epic_counts(issues: &[Issue]) -> HashMap<Uuid, (usize, usize)> {
    let mut counts: HashMap<Uuid, (usize, usize)> = HashMap::new();
    for issue in issues {
        for rel in &issue.relationships {
            if rel.relation_type == RelationType::PartOf {
                let entry = counts.entry(rel.target_id).or_insert((0, 0));
                entry.0 += 1;
                if issue.status == Status::Done {
                    entry.1 += 1;
                }
            }
        }
    }
    counts
}

/// Returns true if adding a directed edge `from -> to` to the adjacency map
/// would create a directed cycle (i.e. `to` can already reach `from`).
/// Pure function used to enforce the DAG invariant (SPEC r[graph-acyclic]).
pub fn creates_cycle(adjacency: &HashMap<Uuid, Vec<Uuid>>, from: Uuid, to: Uuid) -> bool {
    // Walk forward from `to`; reaching `from` means the new edge closes a cycle.
    let mut stack = vec![to];
    let mut visited: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    while let Some(node) = stack.pop() {
        if node == from {
            return true;
        }
        if !visited.insert(node) {
            continue;
        }
        if let Some(nexts) = adjacency.get(&node) {
            stack.extend(nexts.iter().copied());
        }
    }
    false
}

/// Build the dependency adjacency map (edge: source -> target) over the
/// ordering relationships `required_for` and `scheduled_before`. `part_of`
/// is containment, not a dependency, and is excluded.
pub fn build_dependency_adjacency(issues: &[Issue]) -> HashMap<Uuid, Vec<Uuid>> {
    let mut adjacency: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for issue in issues {
        for rel in &issue.relationships {
            if matches!(
                rel.relation_type,
                RelationType::RequiredFor | RelationType::ScheduledBefore
            ) {
                adjacency.entry(issue.id).or_default().push(rel.target_id);
            }
        }
    }
    adjacency
}

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
    fn test_issue_type_legacy_ux_serialization() {
        // On disk the type is written with its canonical spelling.
        assert_eq!(
            serde_json::to_string(&IssueType::UxProblem).unwrap(),
            "\"ux_problem\""
        );

        // The canonical spelling parses...
        assert_eq!(
            serde_json::from_str::<IssueType>("\"ux_problem\"").unwrap(),
            IssueType::UxProblem
        );
        assert_eq!("ux_problem".parse::<IssueType>(), Ok(IssueType::UxProblem));

        // ...and so does the legacy `ux` form written by older versions of
        // pebbles (the CLI shorthand), so existing data keeps loading.
        assert_eq!(
            serde_json::from_str::<IssueType>("\"ux\"").unwrap(),
            IssueType::UxProblem
        );
        assert_eq!("ux".parse::<IssueType>(), Ok(IssueType::UxProblem));
    }

    #[test]
    fn test_build_epic_counts() {
        let mut child_done = make_issue("C1", Status::Done);
        let mut child_todo = make_issue("C2", Status::Todo);
        let epic = make_issue("E", Status::Todo);
        let epic_id = epic.id;

        child_done.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic_id,
        });
        child_todo.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic_id,
        });

        let counts = build_epic_counts(&[child_done, child_todo, epic]);
        assert_eq!(counts.get(&epic_id), Some(&(2, 1)));
    }

    #[test]
    fn test_creates_cycle_detects_reachable_cycle() {
        let mut a = make_issue("A", Status::Todo);
        let mut b = make_issue("B", Status::Todo);
        let c = make_issue("C", Status::Todo);

        // A -> B -> C via required_for.
        a.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: b.id,
        });
        b.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: c.id,
        });
        let adjacency = build_dependency_adjacency(&[a.clone(), b.clone(), c.clone()]);

        // C -> A closes the cycle; A -> C does not.
        assert!(creates_cycle(&adjacency, c.id, a.id));
        assert!(creates_cycle(&adjacency, c.id, b.id)); // C -> B closes B <-> C
        assert!(!creates_cycle(&adjacency, a.id, c.id));
        drop(c);
    }

    #[test]
    fn test_creates_cycle_self_edge() {
        let a = make_issue("A", Status::Todo);
        let adjacency = build_dependency_adjacency(std::slice::from_ref(&a));
        // An edge from A to A is the minimal cycle.
        assert!(creates_cycle(&adjacency, a.id, a.id));
    }

    #[test]
    fn test_build_dependency_adjacency_excludes_part_of() {
        let mut child = make_issue("Child", Status::Todo);
        let epic = make_issue("Epic", Status::Todo);
        child.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic.id,
        });
        let adjacency = build_dependency_adjacency(&[child, epic]);
        assert!(adjacency.is_empty());
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
