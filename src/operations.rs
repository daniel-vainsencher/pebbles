use anyhow::{bail, Context, Result};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::export::{drop_missing_relationships, parse_export_bundle, validate_import};
use crate::jj::JJ;
use crate::models::{
    build_dependency_adjacency, build_epic_counts, build_incoming_relationships_map, creates_cycle,
    IncomingRelationship, Index, Issue, IssueType, RelationType, Relationship, Status,
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

/// In-memory view of the issue set for one invocation: the issues keyed by
/// id plus the derived maps the display queries need (related titles,
/// incoming relationships, epic counts). Built once by `build_project_graph`
/// from the single `storage.load_all_issues` read made by
/// `Operations::load_graph`, then served to every query without further
/// storage reads.
#[derive(Clone)]
pub struct ProjectGraph {
    issues: HashMap<Uuid, Issue>,
    /// Per issue: titles of the targets of its relationships that exist in
    /// the set (targets missing from the set are skipped).
    related_titles: HashMap<Uuid, HashMap<Uuid, String>>,
    /// Per target id: relationships pointing at it, from
    /// `build_incoming_relationships_map`.
    incoming: HashMap<Uuid, Vec<IncomingRelationship>>,
    /// Per epic id: `(total, done)` part_of child counts.
    epic_counts: HashMap<Uuid, (usize, usize)>,
}

impl ProjectGraph {
    /// Titles of the targets of `issue`'s relationships that exist in the
    /// set.
    pub fn related_titles(&self, issue: &Issue) -> HashMap<Uuid, String> {
        self.related_titles
            .get(&issue.id)
            .cloned()
            .unwrap_or_default()
    }

    /// Incoming relationships pointing at `issue_id` as
    /// `(source_id, source_title, relation_type)` tuples.
    pub fn incoming_relationships(&self, issue_id: &Uuid) -> Vec<(Uuid, String, RelationType)> {
        self.incoming
            .get(issue_id)
            .map(|rels| {
                rels.iter()
                    .map(|r| (r.source_id, r.source_title.clone(), r.relation_type.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Per-epic `(total, done)` part_of child counts over the full set.
    pub fn epic_counts(&self) -> &HashMap<Uuid, (usize, usize)> {
        &self.epic_counts
    }

    /// Iterate the full issue set.
    pub fn issues(&self) -> impl Iterator<Item = &Issue> {
        self.issues.values()
    }
}

/// Build the in-memory graph layer for an issue set: index issues by id and
/// derive the maps the display queries need. Pure, no I/O; the caller
/// decides when the set is loaded.
pub fn build_project_graph(issues: &[Issue]) -> ProjectGraph {
    let by_id: HashMap<Uuid, Issue> = issues
        .iter()
        .map(|issue| (issue.id, issue.clone()))
        .collect();
    let related_titles = issues
        .iter()
        .map(|issue| {
            let titles = issue
                .relationships
                .iter()
                .filter_map(|rel| {
                    by_id
                        .get(&rel.target_id)
                        .map(|target| (rel.target_id, target.title.clone()))
                })
                .collect();
            (issue.id, titles)
        })
        .collect();
    ProjectGraph {
        issues: by_id,
        related_titles,
        incoming: build_incoming_relationships_map(issues),
        epic_counts: build_epic_counts(issues),
    }
}

/// The class of one index/disk divergence reported by `pebbles doctor`
/// (SPEC r[index-consistent]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticClass {
    /// Index entry whose issue file does not exist on disk.
    MissingFile,
    /// Issue file on disk with no index entry (the repo's historical
    /// test-pollution files land here).
    OrphanedFile,
    /// Index entry that does not resolve to a valid issue: the entry's
    /// file exists but does not parse as an `Issue`.
    UnknownIndexEntry,
    /// Index entry whose file parses as an issue declaring a different id
    /// than the entry maps (e.g. a file copied over the indexed name).
    FileMismatch,
}

impl DiagnosticClass {
    /// Stable lowercase tag for one diagnostic class: the greppable prefix
    /// of every `pebbles doctor` output line and the sort tie-break.
    pub fn tag(&self) -> &'static str {
        match self {
            DiagnosticClass::MissingFile => "missing",
            DiagnosticClass::OrphanedFile => "orphaned",
            DiagnosticClass::UnknownIndexEntry => "unknown",
            DiagnosticClass::FileMismatch => "mismatch",
        }
    }
}

/// One index/disk divergence reported by `pebbles doctor`. `id` is the
/// index entry's id when the diagnostic concerns an index entry, otherwise
/// `None`; `detail` is a human-readable one-line explanation rendered
/// verbatim by the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub class: DiagnosticClass,
    pub id: Option<Uuid>,
    pub file_name: String,
    pub detail: String,
}

/// Load outcome for one indexed issue file, fed to `doctor` by the
/// operation shell after its single read pass over the issue set: the file
/// name the index maps, plus `Some(id)` when the file parses as an issue
/// (the id it declares) and `None` when it does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileLoad {
    pub file_name: String,
    pub id: Option<Uuid>,
}

/// Result of `pebbles doctor` (SPEC r[index-consistent]): the indexed
/// issue count plus every divergence found. Report only; the repair half
/// is `pebbles reindex` (r[reindex]).
pub struct DoctorOutcome {
    pub total_issues: usize,
    pub diagnostics: Vec<Diagnostic>,
}

/// Detect divergence between `.pebbles/index.json` and `.pebbles/issues/`
/// (SPEC r[index-consistent]): index entries whose file is missing on
/// disk, issue files the index does not know, entries whose file does not
/// parse as a valid issue, and entries whose file declares a different id
/// than the index maps. Pure and deterministic: takes the index, the
/// issue file names on disk, and the per-indexed-file load outcomes, and
/// returns every divergence sorted by file name. Only reports; the repair
/// half is `pebbles reindex` (r[reindex]).
pub fn doctor(index: &Index, disk_files: &[&str], loads: &[IndexedFileLoad]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // One diagnostic per index entry, in index order: unknown (the file
    // exists but does not parse), then mismatch (it parses under a
    // different id), then missing (no file on disk at all). An entry whose
    // file parses as its own id is consistent and stays silent.
    for metadata in &index.issues {
        let Some(load) = loads.iter().find(|l| l.file_name == metadata.file_name) else {
            // The load pass reads only files that exist on disk: no load
            // row means the indexed file is missing.
            diagnostics.push(Diagnostic {
                class: DiagnosticClass::MissingFile,
                id: Some(metadata.id),
                file_name: metadata.file_name.clone(),
                detail: format!("(index entry {}, file not on disk)", metadata.id),
            });
            continue;
        };
        match load.id {
            None => diagnostics.push(Diagnostic {
                class: DiagnosticClass::UnknownIndexEntry,
                id: Some(metadata.id),
                file_name: metadata.file_name.clone(),
                detail: format!(
                    "(index entry {}, file does not parse as an issue)",
                    metadata.id
                ),
            }),
            Some(id) => {
                if id != metadata.id {
                    diagnostics.push(Diagnostic {
                        class: DiagnosticClass::FileMismatch,
                        id: Some(metadata.id),
                        file_name: metadata.file_name.clone(),
                        detail: format!("(index id {}, file declares {})", metadata.id, id),
                    });
                }
            }
        }
    }

    // One diagnostic per issue file on disk that no index entry maps
    // (orphaned files, r[index-consistent]).
    let mut indexed_names = HashSet::new();
    for metadata in &index.issues {
        indexed_names.insert(metadata.file_name.clone());
    }
    for file_name in disk_files {
        let name = file_name.to_string();
        if !indexed_names.contains(&name) {
            diagnostics.push(Diagnostic {
                class: DiagnosticClass::OrphanedFile,
                id: None,
                file_name: name,
                detail: format!("{}", "(file not in index)"),
            });
        }
    }

    // Deterministic output, sorted by file name with the class tag as the
    // tie-break (r[index-consistent]: sorted by id/path, machine-greppable).
    diagnostics.sort_by_key(|d| format!("{}:{}", d.file_name.clone(), d.class.tag()));
    diagnostics
}

/// Render the `pebbles doctor` report: a one-line clean bill of health for
/// a consistent store, or a per-class summary with one indented line per
/// diagnostic (r[index-consistent]). Deterministic: classes appear in the
/// fixed order below and their diagnostics arrive sorted by file name.
pub fn doctor_lines(total_issues: usize, diagnostics: &[Diagnostic]) -> Vec<String> {
    if diagnostics.is_empty() {
        return vec![format!(
            "✓ Store is consistent: {} indexed issue(s); every issue file and index entry match.",
            total_issues
        )];
    }
    let mut lines = vec![format!(
        "✗ Found {} divergence(s) between .pebbles/index.json and .pebbles/issues/:",
        diagnostics.len()
    )];
    let classes = [
        DiagnosticClass::MissingFile,
        DiagnosticClass::OrphanedFile,
        DiagnosticClass::UnknownIndexEntry,
        DiagnosticClass::FileMismatch,
    ];
    for class in &classes {
        let items: Vec<&Diagnostic> = diagnostics.iter().filter(|d| d.class == *class).collect();
        if items.is_empty() {
            continue;
        }
        lines.push(format!("{} ({}):", class.tag(), items.len()));
        for d in items {
            lines.push(format!("  {} {}", d.file_name, d.detail));
        }
    }
    lines
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
    /// The issue-set graph loaded once per invocation by `load_graph`;
    /// served to `get_related_titles` / `get_incoming_relationships` so
    /// display commands make a single `load_all_issues` read.
    graph: RefCell<Option<(ProjectGraph, Vec<LoadWarning>)>>,
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
/// relationship from a non-Done issue (SPEC `next`). An `Epic` with any
/// unblocked `part_of` child is dropped: those children already represent
/// the work. Sorted by `updated_at` descending (r[impl issue-ordering]).
/// Pure: no storage access, so the blocking rules are unit-testable
/// without I/O.
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

    // An epic is noise in `next` when any of its `part_of` children are
    // themselves unblocked: those children already represent the
    // actionable work. An epic with no unblocked children (all blocked,
    // or none yet) stays as a placeholder.
    let unblocked_ids: HashSet<Uuid> = unblocked_issues.iter().map(|i| i.id).collect();
    unblocked_issues.retain(|issue| {
        if issue.issue_type != IssueType::Epic {
            return true;
        }
        !incoming_map.get(&issue.id).is_some_and(|incoming| {
            incoming.iter().any(|rel| {
                rel.relation_type == RelationType::PartOf && unblocked_ids.contains(&rel.source_id)
            })
        })
    });

    // r[impl issue-ordering]
    unblocked_issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

    unblocked_issues
}

/// Per-issue dependency/blocker status for `pebbles list` (SPEC
/// r[blocker-indicators]).
///
/// A `required_for` edge from A to B means B requires A to be done before
/// it can start: A is a prerequisite of B. "Blocked" here is the opposite
/// direction of the classic relation names and matches `next`: B is blocked
/// by its still-open prerequisites.
/// - `blocked_by` lists the non-Done issues this issue depends on (the
///   sources of its incoming `required_for` edges); empty means the issue
///   is not blocked.
/// - `blocking` lists the non-Done issues that depend on this one (the
///   targets of its outgoing `required_for` edges); informational only,
///   and empty once this issue is Done.
/// - `ready` is true when `blocked_by` is empty, i.e. the issue has no
///   incomplete `required_for` dependencies.
/// - `circular` is true when the issue sits on a `required_for` or
///   `scheduled_before` cycle (a self-reference is the minimal cycle);
///   `pebbles link` rejects such edges (r[graph-acyclic]), so this flags
///   data that reached the store some other way (manual edit + reindex,
///   import).
///
/// Only `required_for` counts as a blocker here (`part_of` is containment
/// and never a prerequisite; `scheduled_before` is a scheduling hint, not
/// a prerequisite). Both lists deduplicate; `blocked_by` keeps incoming
/// relationship order, `blocking` is sorted by ID.
///
/// Pure: no storage access, so the indicator computation is unit-testable
/// without I/O (ARCHITECTURE: functional core, imperative shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerStatus {
    /// True when the issue has no incomplete `required_for` dependencies.
    pub ready: bool,
    /// IDs of the non-Done prerequisites this issue depends on (sources of
    /// its incoming `required_for` edges), deduplicated in edge order.
    pub blocked_by: Vec<Uuid>,
    /// IDs of the non-Done issues that depend on this one (targets of its
    /// outgoing `required_for` edges), sorted and deduplicated. Empty when
    /// this issue is itself Done (nothing is still waiting on it).
    pub blocking: Vec<Uuid>,
    /// True when the issue is part of a circular dependency (SPEC
    /// r[blocker-colors]).
    pub circular: bool,
}

/// The IDs of every issue that participates in a dependency cycle.
///
/// The dependency graph follows `build_dependency_adjacency`
/// (`required_for` + `scheduled_before` edges); an issue is circular when
/// it can reach itself (a self-referencing edge is the minimal cycle).
/// `pebbles link` rejects edges that would close a cycle (r[graph-acyclic]),
/// so circular data reaches the store only through other means (manual
/// edit + reindex, import).
///
/// Pure: no storage access, so the cycle computation is unit-testable
/// without I/O (ARCHITECTURE: functional core, imperative shell).
pub fn build_circular_issues(issues: &[Issue]) -> HashSet<Uuid> {
    let adjacency = build_dependency_adjacency(issues);
    let mut circular: HashSet<Uuid> = HashSet::new();
    for issue in issues {
        // Walk forward from the issue; reaching it again means a cycle.
        let mut stack = vec![issue.id];
        let mut visited: HashSet<Uuid> = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == issue.id && !visited.is_empty() {
                circular.insert(issue.id);
                break;
            }
            if !visited.insert(node) {
                continue;
            }
            if let Some(nexts) = adjacency.get(&node) {
                stack.extend(nexts.iter().copied());
            }
        }
    }
    circular
}

/// Compute the dependency/blocker status of every issue in `issues`.
///
/// Returns a map keyed by issue ID. Issues with no outgoing `required_for`
/// relationships get an empty `blocked_by` (ready); issues with no incoming
/// `required_for` from non-Done sources get an empty `blocking` list.
pub fn build_blocker_status(issues: &[Issue]) -> HashMap<Uuid, BlockerStatus> {
    let incoming_map = build_incoming_relationships_map(issues);
    let circular_ids = build_circular_issues(issues);
    let mut statuses: HashMap<Uuid, BlockerStatus> = HashMap::new();

    for issue in issues {
        // Blocked by every non-Done issue whose `required_for` points at
        // this one (incoming edges): those are the prerequisites this issue
        // depends on and that are still open.
        let mut blocked_by: Vec<Uuid> = incoming_map
            .get(&issue.id)
            .map(|incoming| {
                incoming
                    .iter()
                    .filter(|rel| rel.relation_type == RelationType::RequiredFor)
                    .filter(|rel| rel.source_status != Status::Done)
                    .map(|rel| rel.source_id)
                    .collect()
            })
            .unwrap_or_default();
        blocked_by.sort_unstable();
        blocked_by.dedup();

        // Blocks every non-Done issue that this issue's `required_for`
        // points at (outgoing edges): those depend on this issue.
        let mut blocking: Vec<Uuid> = issue
            .relationships
            .iter()
            .filter(|_| issue.status != Status::Done) // done blocks nothing
            .filter(|rel| rel.relation_type == RelationType::RequiredFor)
            .filter(|rel| {
                issues
                    .iter()
                    .find(|other| other.id == rel.target_id)
                    .map(|other| other.status != Status::Done)
                    .unwrap_or(true) // dangling targets count as incomplete
            })
            .map(|rel| rel.target_id)
            .collect();
        let mut seen = HashSet::new();
        blocking.retain(|id| seen.insert(*id));

        statuses.insert(
            issue.id,
            BlockerStatus {
                ready: blocked_by.is_empty(),
                blocked_by,
                blocking,
                circular: circular_ids.contains(&issue.id),
            },
        );
    }
    statuses
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

/// A single term of a `pebbles list --grep` full-text query
/// (r[grep-query-syntax]). Terms are lowercased at parse time, so matching
/// is always case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextQueryTerm {
    /// A bare, unquoted term: matched as a case-insensitive substring of
    /// the issue's title or description.
    Substring(String),
    /// A double-quoted phrase: matched verbatim (internal whitespace
    /// preserved, word order fixed) as a case-insensitive substring of the
    /// issue's title or description.
    Phrase(String),
}

/// Parse a `--grep` query into ANDed terms (r[grep-query-syntax]).
///
/// Syntax:
/// - Space-separated terms outside quotes become [`TextQueryTerm::Substring`]
///   terms, ANDed together: every term must match.
/// - A double-quoted segment becomes a [`TextQueryTerm::Phrase`]: it matches
///   verbatim (internal whitespace preserved), still case-insensitively.
/// - A blank (empty or whitespace-only) query parses to no terms, which
///   matches nothing (r[continuity]) -- the same convention as
///   `--search ""` (see `filter_by_title_query`).
///
/// Malformed queries are an error, not a silent fallback (r[grep-query-parse]):
/// - an unterminated quote (a `"` with no closing `"`), and
/// - an empty phrase (`""`), which would match every issue (r[continuity]).
///
/// Pure: no storage access, so the parsing is unit-testable without I/O
/// (ARCHITECTURE: functional core, imperative shell).
///
/// ```rust
/// # use pebbles::operations::{parse_text_query, TextQueryTerm};
/// let terms = parse_text_query("login \"jwt token\"").unwrap();
/// assert_eq!(
///     terms,
///     vec![
///         TextQueryTerm::Substring("login".to_string()),
///         TextQueryTerm::Phrase("jwt token".to_string()),
///     ]
/// );
/// ```
pub fn parse_text_query(query: &str) -> Result<Vec<TextQueryTerm>> {
    let mut terms: Vec<TextQueryTerm> = Vec::new();
    let mut bare = String::new();
    let mut in_phrase = false;
    let mut phrase = String::new();

    for ch in query.trim().chars() {
        if in_phrase {
            if ch == '"' {
                in_phrase = false;
                if phrase.is_empty() {
                    bail!("Empty phrase in --grep query: \"\" is not a selector");
                }
                terms.push(TextQueryTerm::Phrase(phrase.to_lowercase()));
                phrase.clear();
            } else {
                phrase.push(ch);
            }
            continue;
        }
        if ch == '"' {
            flush_bare_term(&mut terms, &mut bare);
            in_phrase = true;
        } else if ch.is_whitespace() {
            flush_bare_term(&mut terms, &mut bare);
        } else {
            bare.push(ch);
        }
    }
    if in_phrase {
        bail!("Unterminated quote in --grep query: missing closing \"");
    }
    flush_bare_term(&mut terms, &mut bare);
    Ok(terms)
}

/// Append the accumulated bare term (lowercased) to `terms`, if any.
fn flush_bare_term(terms: &mut Vec<TextQueryTerm>, bare: &mut String) {
    if !bare.is_empty() {
        terms.push(TextQueryTerm::Substring(
            std::mem::take(bare).to_lowercase(),
        ));
    }
}

/// The issues from `issues` whose title or description contains every term
/// of `terms`, in input order (r[grep-query-syntax]): `--grep` is a boolean
/// filter and adds no relevance ranking, unlike `--search` which sorts by
/// fuzzy-match quality.
///
/// Matching is a case-insensitive substring check, done once per issue on
/// the lowercased title and description; an issue without a description has
/// only its title to match. An empty `terms` slice matches nothing
/// (r[continuity]): `--grep ""` is a distinct filter, not the unfiltered
/// list.
///
/// Pure: no storage access, so the matching and ordering logic is
/// unit-testable without I/O (ARCHITECTURE: functional core, imperative
/// shell).
pub fn filter_by_text_query(issues: &[Issue], terms: &[TextQueryTerm]) -> Vec<Issue> {
    if terms.is_empty() {
        return Vec::new();
    }
    issues
        .iter()
        .filter(|issue| {
            let mut haystack = issue.title.to_lowercase();
            if let Some(description) = &issue.description {
                haystack.push('\n');
                haystack.push_str(&description.to_lowercase());
            }
            terms.iter().all(|term| {
                let needle = match term {
                    TextQueryTerm::Substring(n) | TextQueryTerm::Phrase(n) => n,
                };
                haystack.contains(needle)
            })
        })
        .cloned()
        .collect()
}

/// Aggregate `issues` into a `ProjectSummary`: total count, counts by
/// status and type, per-epic child progress (`part_of` children, done
/// count), the actionable (unblocked todo) count, and relationship totals
/// by relation type. Pure: no storage access, so the aggregation and the
/// actionability link to `get_unblocked_issues` are unit-testable without
/// I/O.
/// Reject tag inputs that would store a meaningless value: any tag that is
/// empty or whitespace-only (after trimming) is an error. Called by the
/// mutating tag commands and by `list --tag` BEFORE any resolution or write,
/// so a bad tag fails the whole request atomically. Pure: no storage access.
///
/// ```rust
/// # use pebbles::operations::validate_tags;
/// assert!(validate_tags(&["backend".to_string()]).is_ok());
/// assert!(validate_tags(&["backend".to_string(), "  ".to_string()]).is_err());
/// assert!(validate_tags(&["".to_string()]).is_err());
/// ```
pub fn validate_tags(tags: &[String]) -> Result<()> {
    if let Some(bad) = tags.iter().find(|t| t.trim().is_empty()) {
        bail!("Tag cannot be empty (got {:?})", bad);
    }
    Ok(())
}

/// Append `tags` to `issue.tags` in input order, deduplicating against the
/// tags already present and against earlier entries in `tags` themselves
/// (r[tag-add-dedupe]). Matching is exact and case-sensitive: tags are
/// stored as given. Returns the tags actually appended (in input order);
/// empty when nothing changed. Pure: no storage access, so the
/// append/dedupe rules are unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::add_tags_to_issue;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// let added = add_tags_to_issue(&mut issue, &["x".to_string(), "x".to_string(), "y".to_string()]);
/// assert_eq!(added, vec!["x".to_string(), "y".to_string()]);
/// assert_eq!(issue.tags, vec!["x".to_string(), "y".to_string()]);
/// assert!(add_tags_to_issue(&mut issue, &["x".to_string()]).is_empty());
/// ```
// r[impl tag-add-dedupe]
pub fn add_tags_to_issue(issue: &mut Issue, tags: &[String]) -> Vec<String> {
    let mut added = Vec::new();
    for tag in tags {
        if !issue.tags.iter().any(|existing| existing == tag) {
            issue.tags.push(tag.clone());
            added.push(tag.clone());
        }
    }
    added
}

/// Remove every occurrence of `tags` from `issue.tags` (exact,
/// case-sensitive match), keeping the remaining tags in their current
/// order. Tags that were not present are ignored. Returns the tags actually
/// removed (in request order, without duplicates); empty when nothing
/// changed. Pure: no storage access, so the removal rules are
/// unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::remove_tags_from_issue;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// issue.tags = vec!["x".to_string(), "y".to_string()];
/// let removed = remove_tags_from_issue(&mut issue, &["y".to_string(), "nope".to_string()]);
/// assert_eq!(removed, vec!["y".to_string()]);
/// assert_eq!(issue.tags, vec!["x".to_string()]);
/// ```
pub fn remove_tags_from_issue(issue: &mut Issue, tags: &[String]) -> Vec<String> {
    let mut removed = Vec::new();
    for tag in tags {
        if issue.tags.iter().any(|existing| existing == tag) {
            issue.tags.retain(|existing| existing != tag);
            removed.push(tag.clone());
        }
    }
    removed
}

/// Append `addition` to a description as a new line, matching the semantics
/// of `pebbles update --append-description` (r[update-append]). Pure: no
/// storage access, so the append rules are unit-testable without I/O.
///
/// Rules:
/// - no current description, or an empty one -> `addition` verbatim
/// - current description ends with a newline -> current + addition
/// - otherwise -> current + "\n" + addition (append a line)
/// - an empty `addition` leaves the description unchanged (a no-op)
///
/// ```rust
/// # use pebbles::operations::append_description_text;
/// assert_eq!(append_description_text(None, "line"), "line");
/// assert_eq!(append_description_text(Some(""), "line"), "line");
/// assert_eq!(append_description_text(Some("a"), "b"), "a\nb");
/// assert_eq!(append_description_text(Some("a\n"), "b"), "a\nb");
/// assert_eq!(append_description_text(Some("a"), ""), "a");
/// assert_eq!(append_description_text(None, ""), "");
/// ```
pub fn append_description_text(description: Option<&str>, addition: &str) -> String {
    if addition.is_empty() {
        return description.unwrap_or_default().to_string();
    }
    match description {
        None | Some("") => addition.to_string(),
        Some(current) if current.ends_with('\n') => format!("{}{}", current, addition),
        Some(current) => format!("{}\n{}", current, addition),
    }
}

/// The issues from `issues` that carry EVERY tag in `filter` (exact,
/// case-sensitive match against the stored `tags`; r[tag-filter-and]).
/// Input order is preserved: the caller produces a sorted list and this
/// retains the slice. An empty filter returns every issue (vacuous AND);
/// the CLI rejects empty `--tag` values before this is reachable. Pure: no
/// storage access, so the filter rules are unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::filter_by_tags;
/// let mut a = Issue::new("A".to_string(), None, IssueType::Bug);
/// let mut b = Issue::new("B".to_string(), None, IssueType::Bug);
/// a.tags = vec!["x".to_string(), "y".to_string()];
/// b.tags = vec!["x".to_string()];
/// let out = filter_by_tags(&[a, b], &["x".to_string(), "y".to_string()]);
/// assert_eq!(out.len(), 1);
/// assert_eq!(out[0].title, "A");
/// ```
// r[tag-filter-and]
pub fn filter_by_tags(issues: &[Issue], filter: &[String]) -> Vec<Issue> {
    issues
        .iter()
        .filter(|issue| filter.iter().all(|tag| issue.tags.contains(tag)))
        .cloned()
        .collect()
}

/// Reject metadata keys that would store a meaningless selector: a key that
/// is empty or whitespace-only (after trimming) is an error. Called by the
/// mutating metadata commands and by `list --metadata` BEFORE any resolution
/// or write, so a bad key fails the whole request atomically. Values are
/// free-form strings and may be empty; only the key identifies the field
/// (r[metadata-set-upsert]). Pure: no storage access.
///
/// ```rust
/// # use pebbles::operations::validate_metadata_key;
/// assert!(validate_metadata_key("priority").is_ok());
/// assert!(validate_metadata_key("").is_err());
/// assert!(validate_metadata_key("  ").is_err());
/// ```
pub fn validate_metadata_key(key: &str) -> Result<()> {
    if key.trim().is_empty() {
        bail!("Metadata key cannot be empty");
    }
    Ok(())
}

/// Parse one `--metadata KEY=VALUE` filter into its (key, value) pair
/// (r[metadata-filter-parse]). The pair is split at the FIRST `=` so values
/// may contain `=`; both sides are trimmed on entry and the key is
/// validated. A pair without `=` or with a blank key is an error: it is not
/// a selector for anything (r[continuity]). Pure: no storage access.
///
/// ```rust
/// # use pebbles::operations::parse_metadata_filter;
/// let pair = parse_metadata_filter("priority=high").unwrap();
/// assert_eq!(pair, ("priority".to_string(), "high".to_string()));
/// assert!(parse_metadata_filter("priority").is_err());
/// assert!(parse_metadata_filter("=high").is_err());
/// ```
pub fn parse_metadata_filter(input: &str) -> Result<(String, String)> {
    let (key, value) = input
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("Metadata filter must be KEY=VALUE (got {:?})", input))?;
    let key = key.trim().to_string();
    let value = value.trim().to_string();
    validate_metadata_key(&key)?;
    Ok((key, value))
}

/// Set `key` to `value` in `issue.metadata`, upserting: a new key is
/// inserted and an existing key's value is replaced in place
/// (r[metadata-set-upsert]). Keys and values are expected pre-trimmed
/// (callers validate the key first). Returns true when the map changed (new
/// key or different value); false when the key already carried exactly this
/// value, so the caller can report no changes and write nothing. Empty
/// values are allowed: `set` with an empty value is a presence marker, not
/// an error. Pure: no storage access, so the upsert rules are unit-testable
/// without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::set_metadata_on_issue;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// assert!(set_metadata_on_issue(&mut issue, "priority", "high"));
/// assert!(!set_metadata_on_issue(&mut issue, "priority", "high"));
/// assert_eq!(issue.metadata.get("priority").map(String::as_str), Some("high"));
/// ```
// r[impl metadata-set-upsert]
pub fn set_metadata_on_issue(issue: &mut Issue, key: &str, value: &str) -> bool {
    match issue.metadata.get(key) {
        Some(existing) if existing == value => false,
        _ => {
            issue.metadata.insert(key.to_string(), value.to_string());
            true
        }
    }
}

/// Remove `key` from `issue.metadata` (exact, case-sensitive match). Returns
/// true when the key was present and removed; false when it was absent (a
/// silent no-op, r[metadata-unset-absent]). Keys are expected pre-trimmed
/// (the caller validates the key first). Pure: no storage access, so the
/// removal rules are unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::unset_metadata_on_issue;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// assert!(!unset_metadata_on_issue(&mut issue, "priority"));
/// ```
// r[impl metadata-unset-absent]
pub fn unset_metadata_on_issue(issue: &mut Issue, key: &str) -> bool {
    issue.metadata.remove(key).is_some()
}

/// The issues from `issues` whose metadata carries EVERY (key, value) pair
/// in `filters` (exact, case-sensitive match on both key and value;
/// r[metadata-filter-and]). Input order is preserved: the caller produces a
/// sorted list and this retains the slice. An empty filter returns every
/// issue (vacuous AND); the CLI rejects malformed `--metadata` values before
/// this is reachable. Pure: no storage access, so the filter rules are
/// unit-testable without I/O.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::filter_by_metadata;
/// # use pebbles::operations::set_metadata_on_issue;
/// let mut a = Issue::new("A".to_string(), None, IssueType::Bug);
/// let mut b = Issue::new("B".to_string(), None, IssueType::Bug);
/// set_metadata_on_issue(&mut a, "priority", "high");
/// set_metadata_on_issue(&mut b, "priority", "low");
/// let out = filter_by_metadata(&[a, b], &[("priority".to_string(), "high".to_string())]);
/// assert_eq!(out.len(), 1);
/// assert_eq!(out[0].title, "A");
/// ```
// r[metadata-filter-and]
pub fn filter_by_metadata(issues: &[Issue], filters: &[(String, String)]) -> Vec<Issue> {
    issues
        .iter()
        .filter(|issue| {
            filters.iter().all(|(key, value)| {
                issue.metadata.get(key).map(String::as_str) == Some(value.as_str())
            })
        })
        .cloned()
        .collect()
}

/// The issue's metadata as (key, value) pairs sorted by key, for
/// deterministic display: HashMap iteration order is unspecified, and
/// `metadata get <id>` must print the same order every run
/// (r[metadata-get-sorted]). Pure: no storage access.
///
/// ```rust
/// # use pebbles::models::{Issue, IssueType};
/// # use pebbles::operations::sorted_metadata;
/// # use pebbles::operations::set_metadata_on_issue;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// set_metadata_on_issue(&mut issue, "zeta", "1");
/// set_metadata_on_issue(&mut issue, "alpha", "2");
/// assert_eq!(sorted_metadata(&issue)[0].0, "alpha");
/// ```
// r[impl metadata-get-sorted]
pub fn sorted_metadata(issue: &Issue) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = issue
        .metadata
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// One prompted field of a type-specific creation template: the metadata
/// key the answer is stored under (`key`) and the prompt text shown to the
/// user (`label`). See `template_fields_for` (r[type-templates]).
#[derive(Debug, Clone)]
pub struct TemplateField {
    pub key: String,
    pub label: String,
}

/// The template fields prompted for when creating an issue of
/// `issue_type` interactively (stdin is a terminal). Each field's `key` is
/// the metadata key the answer is stored under (r[metadata-set-upsert]);
/// types without a field set prompt nothing and create exactly as before.
/// The prompts are interactive-only: when stdin is not a terminal the flow
/// skips them silently, so piped and scripted creates never block or print
/// prompt text (r[type-templates]). Pure: no storage access and no I/O, so
/// field sets are unit-testable without a terminal.
///
/// ```rust
/// # use pebbles::models::IssueType;
/// # use pebbles::operations::template_fields_for;
/// assert_eq!(template_fields_for(IssueType::UxProblem).len(), 2);
/// assert_eq!(template_fields_for(IssueType::Bug)[0].key, "expected");
/// assert!(template_fields_for(IssueType::Epic).is_empty());
/// ```
// r[impl type-templates]
pub fn template_fields_for(issue_type: IssueType) -> Vec<TemplateField> {
    match issue_type {
        IssueType::Bug => vec![
            TemplateField {
                key: "expected".to_string(),
                label: "Expected behavior".to_string(),
            },
            TemplateField {
                key: "actual".to_string(),
                label: "Actual behavior".to_string(),
            },
        ],
        IssueType::UxProblem => vec![
            TemplateField {
                key: "before_screenshot".to_string(),
                label: "Before screenshot (path or URL)".to_string(),
            },
            TemplateField {
                key: "after_screenshot".to_string(),
                label: "After screenshot (path or URL)".to_string(),
            },
        ],
        IssueType::Usecase => vec![TemplateField {
            key: "acceptance_criteria".to_string(),
            label: "Acceptance criteria".to_string(),
        }],
        IssueType::Design | IssueType::Validation | IssueType::Epic | IssueType::Milestone => {
            Vec::new()
        }
    }
}

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

// r[guidance-tips]
/// The optional contextual guidance to show after a successful issue
/// creation. Selection is a pure decision (`select_creation_tips`), so the
/// rules are unit-testable without I/O (ARCHITECTURE: functional core,
/// imperative shell); `display::print_creation_tips` renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreationGuidance {
    /// Suggest linking the new issue into an existing epic with `part_of`.
    pub suggest_epic: bool,
    /// Existing epics as (id, title), offered so the user can pick a
    /// `part_of` target. Populated exactly when `suggest_epic` holds.
    pub epic_candidates: Vec<(Uuid, String)>,
    /// Suggest declaring a `required_for` dependency on another issue.
    pub suggest_dependency: bool,
}

/// Decide which optional tips to show after creating an issue
/// (r[guidance-tips]).
///
/// Rules:
/// - Epic tip: the new issue's type can be part of an epic (every type but
///   `Epic` and `Milestone`, r[part-of-epic-types]), the project already
///   has epics to link into, and the issue is not already part of one.
/// - Dependency tip: the issue declares no relationships yet.
/// - Both tips are gated on the project having at least one epic: with no
///   epics there is no pickable `part_of` target, and the suggestions
///   would be noise for a project that has not adopted that structure yet.
///
/// Tips are hints, never errors: they carry no failure semantics for the
/// creation itself.
///
/// ```rust
/// # use pebbles::models::{IssueType, RelationType, Relationship};
/// # use pebbles::operations::select_creation_tips;
/// # use uuid::Uuid;
/// let epics: Vec<(Uuid, String)> = vec![(
///     Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000001").unwrap(),
///     "User Auth".to_string(),
/// )];
/// let guidance = select_creation_tips(&IssueType::Bug, &Vec::new(), &epics);
/// assert!(guidance.suggest_epic);
/// assert_eq!(guidance.epic_candidates, epics);
/// assert!(guidance.suggest_dependency);
/// let mut rels: Vec<Relationship> = vec![Relationship {
///     relation_type: RelationType::PartOf,
///     target_id: epics[0].0,
/// }];
/// let linked = select_creation_tips(&IssueType::Bug, &rels, &epics);
/// assert!(!linked.suggest_epic);
/// assert!(!linked.suggest_dependency);
/// ```
pub fn select_creation_tips(
    issue_type: &IssueType,
    relationships: &[Relationship],
    epics: &[(Uuid, String)],
) -> CreationGuidance {
    let already_part_of_an_epic = relationships
        .iter()
        .any(|rel| rel.relation_type == RelationType::PartOf);
    let suggest_epic =
        issue_type.can_be_part_of_epic() && !epics.is_empty() && !already_part_of_an_epic;
    let suggest_dependency = relationships.is_empty() && !epics.is_empty();
    CreationGuidance {
        suggest_epic,
        epic_candidates: if suggest_epic {
            epics.to_vec()
        } else {
            Vec::new()
        },
        suggest_dependency,
    }
}

impl Operations {
    pub fn new(repo_path: &Path) -> Result<Self> {
        let storage = Storage::new(repo_path)?;
        let jj = JJ::new(repo_path)?;

        Ok(Self {
            storage,
            jj,
            graph: RefCell::new(None),
        })
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
        metadata: BTreeMap<String, String>,
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

        // r[impl type-templates] - type-specific template answers (captured
        // interactively by the `new` flow) land in the issue's existing
        // metadata map; no model change (r[metadata-file-order]).
        for (key, value) in metadata {
            set_metadata_on_issue(&mut issue, &key, &value);
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

        // Reload the invocation's graph so the listing is fresh (a long-
        // lived TUI session re-lists after every mutation and external
        // change) and the blocker/epic markers that follow reuse this
        // single load (r[187 graph layer]).
        let (graph, warnings) = self.load_graph()?;
        let mut issues: Vec<Issue> = graph.issues().cloned().collect();

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
        // Serve the target lookup from a fresh single-load graph: show must
        // always reflect the store (a long-lived TUI session displays issues
        // after every mutation), so this reloads the set rather than reusing
        // the cache, and the caller's related/incoming queries reuse the same
        // load (r[187 graph layer]).
        let (graph, _) = self.load_graph()?;
        let issue = match graph.issues().find(|candidate| candidate.id == uuid) {
            Some(candidate) => candidate.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&uuid)?,
        };
        Ok(issue)
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
        append_description: Option<String>,
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

        if let Some(addition) = append_description {
            // Appending the empty string is a no-op: no file write, no jj
            // commit, and the caller reports "No changes: nothing to update".
            if !addition.is_empty() {
                let combined =
                    append_description_text(issue.description.as_deref(), addition.as_str());
                if issue.description.as_deref() != Some(combined.as_str()) {
                    issue.description = Some(combined);
                    changed = true;
                }
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

    /// Append tags to an issue (`pebbles tag add`): resolve and load the
    /// issue, reject empty tags up front (nothing is written on a bad tag),
    /// apply the pure append/dedupe rule, then save and commit. Returns the
    /// tags actually added; empty when every requested tag was already
    /// present (no file write, no jj commit).
    pub fn add_tags(&self, id: &str, tags: &[String]) -> Result<MutationOutcome<Vec<String>>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        // Trim and validate before touching anything: a single bad tag
        // fails the whole request atomically, with nothing added.
        let tags: Vec<String> = tags.iter().map(|t| t.trim().to_string()).collect();
        validate_tags(&tags)?;

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let added = add_tags_to_issue(&mut issue, &tags);
        if added.is_empty() {
            return Ok(MutationOutcome {
                value: Vec::new(),
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
            &format!(
                "Add tags to {}: {} ({})",
                issue.issue_type,
                issue.title,
                added.join(", ")
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: added,
            warnings,
        })
    }

    /// Remove tags from an issue (`pebbles tag remove`): resolve and load
    /// the issue, reject empty tags up front, apply the pure removal rule
    /// (missing tags are ignored), then save and commit. Returns the tags
    /// actually removed; empty when none of the requested tags were present
    /// (no file write, no jj commit).
    pub fn remove_tags(&self, id: &str, tags: &[String]) -> Result<MutationOutcome<Vec<String>>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let tags: Vec<String> = tags.iter().map(|t| t.trim().to_string()).collect();
        validate_tags(&tags)?;

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let removed = remove_tags_from_issue(&mut issue, &tags);
        if removed.is_empty() {
            return Ok(MutationOutcome {
                value: Vec::new(),
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
            &format!(
                "Remove tags from {}: {} ({})",
                issue.issue_type,
                issue.title,
                removed.join(", ")
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: removed,
            warnings,
        })
    }

    /// Set one metadata key on an issue (`pebbles metadata set`): resolve
    /// and load the issue, reject an empty key up front (nothing is written
    /// on a bad key), apply the pure upsert rule, then save and commit.
    /// Returns true when the metadata changed (new key or different value);
    /// false when the key already carried exactly this value (no file write,
    /// no jj commit). Values are trimmed on entry but may be empty.
    pub fn set_metadata(&self, id: &str, key: &str, value: &str) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        // Trim and validate before touching anything: an empty key fails the
        // whole request atomically, with nothing written.
        let key = key.trim().to_string();
        let value = value.trim().to_string();
        validate_metadata_key(&key)?;

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let changed = set_metadata_on_issue(&mut issue, &key, &value);
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
            &format!(
                "Set metadata on {}: {} ({} = {})",
                issue.issue_type, issue.title, key, value
            ),
            &as_path_refs(&paths),
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: true,
            warnings,
        })
    }

    /// Remove one metadata key from an issue (`pebbles metadata unset`):
    /// resolve and load the issue, apply the pure removal rule (an absent
    /// key is a silent no-op), then save and commit. Returns true when the
    /// key was present and removed; false when it was absent (no file write,
    /// no jj commit).
    pub fn unset_metadata(&self, id: &str, key: &str) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let key = key.trim().to_string();
        validate_metadata_key(&key)?;

        let uuid = self.resolve_id(id)?;
        let mut issue = self.storage.load_issue(&uuid)?;

        let removed = unset_metadata_on_issue(&mut issue, &key);
        if !removed {
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
            &format!(
                "Remove metadata from {}: {} ({})",
                issue.issue_type, issue.title, key
            ),
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
        // Serve the query phase (target lookup + broken-link guards) from
        // the invocation's single-load graph; the save path below writes
        // through Storage (r[187 graph layer]).
        let (graph, _) = self.graph_or_load()?;
        let issue = match graph.issues().find(|candidate| candidate.id == uuid) {
            Some(candidate) => candidate.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&uuid)?,
        };
        let issue_path = self.storage.issue_path(&issue.file_name());

        // Find every issue with a relationship pointing at this one. The
        // referencing set is served from the graph (no second issue-set
        // read); the index supplies the store order so the guarded-delete
        // listing stays byte-identical.
        let referencing_by_id: HashMap<Uuid, Issue> = graph
            .issues()
            .filter(|i| i.relationships.iter().any(|rel| rel.target_id == uuid))
            .map(|i| (i.id, i.clone()))
            .collect();
        let referencing: Vec<Issue> = self
            .storage
            .load_index()?
            .issues
            .iter()
            .filter_map(|metadata| referencing_by_id.get(&metadata.id).cloned())
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

        // Serve the query phase (source/target lookup + DAG check) from the
        // invocation's single-load graph; the save path below writes
        // through Storage (r[187 graph layer]).
        let (graph, _) = self.graph_or_load()?;
        let mut source_issue = match graph.issues().find(|i| i.id == source_uuid) {
            Some(issue) => issue.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&source_uuid)?,
        };
        let target_issue = match graph.issues().find(|i| i.id == target_uuid) {
            Some(issue) => issue.clone(),
            None => self.storage.load_issue(&target_uuid)?,
        };

        // DAG invariant: reject edges that close a directed dependency cycle
        // (r[graph-acyclic]).
        let project_issues: Vec<Issue> = graph.issues().cloned().collect();
        let adjacency = build_dependency_adjacency(&project_issues);
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

        // Serve the query phase (source/target lookup) from the
        // invocation's single-load graph; the save path below writes
        // through Storage (r[187 graph layer]).
        let (graph, _) = self.graph_or_load()?;
        let mut source_issue = match graph.issues().find(|i| i.id == source_uuid) {
            Some(issue) => issue.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&source_uuid)?,
        };
        // The target may no longer exist (e.g. it was deleted while this
        // relationship survived); fall back to its short id for the message.
        let target_label = match graph.issues().find(|i| i.id == target_uuid) {
            Some(issue) => issue.title.clone(),
            None => self
                .storage
                .load_issue(&target_uuid)
                .map(|issue| issue.title)
                .unwrap_or_else(|_| target_uuid.to_string()[..8].to_string()),
        };

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
        // Serve the epic lookup and its child search from one fresh load of
        // the set (epic is a display command, so load_graph reloads rather
        // than reusing the cache; the caller surfaces its warnings).
        let (graph, warnings) = self.load_graph()?;
        let epic = match graph.issues().find(|candidate| candidate.id == uuid) {
            Some(candidate) => candidate.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&uuid)?,
        };

        if epic.issue_type != IssueType::Epic {
            let short = self.appropriate_length_id(&uuid)?;
            bail!(
                "Issue {} is not an epic (it's a {})",
                short,
                epic.issue_type
            );
        }

        let mut child_issues: Vec<Issue> = graph
            .issues()
            .filter(|issue| {
                issue
                    .relationships
                    .iter()
                    .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == uuid)
            })
            .cloned()
            .collect();

        // r[impl issue-ordering]
        child_issues.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        Ok((epic, child_issues, warnings))
    }

    pub fn next(&self) -> Result<(Vec<Issue>, Vec<LoadWarning>)> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        // Reload the invocation's graph (fresh read; the blockers that
        // follow reuse it, r[187 graph layer]).
        let (graph, warnings) = self.load_graph()?;
        let all_issues: Vec<Issue> = graph.issues().cloned().collect();
        Ok((get_unblocked_issues(&all_issues), warnings))
    }

    /// Returns `Ok(false)` when the issue already has the requested type
    /// (no file write, no jj commit); the caller warns the user.
    pub fn convert(&self, id: &str, new_type: IssueType) -> Result<MutationOutcome<bool>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let uuid = self.resolve_id(id)?;
        // Serve the query phase (target lookup + epic-child checks) from
        // the invocation's single-load graph; the save path below writes
        // through Storage (r[187 graph layer]).
        let (graph, _) = self.graph_or_load()?;
        let mut issue = match graph.issues().find(|candidate| candidate.id == uuid) {
            Some(candidate) => candidate.clone(),
            // The graph skips issues whose files fail to load (they surface
            // as load warnings); keep the exact `load_issue` errors for
            // that inconsistent-store path.
            None => self.storage.load_issue(&uuid)?,
        };

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
            let has_children = graph.issues().any(|i| {
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

        let (graph, warnings) = self.load_graph()?;
        let all_issues: Vec<Issue> = graph.issues().cloned().collect();
        Ok((calculate_summary(&all_issues), warnings))
    }

    /// Epic child progress `(total, done)` computed over the FULL issue set,
    /// independent of any view filter. See SPEC r[epic-progress-counts].
    pub fn epic_counts(&self) -> Result<HashMap<Uuid, (usize, usize)>> {
        let (graph, _) = self.graph_or_load()?;
        Ok(graph.epic_counts().clone())
    }

    /// Dependency/blocker status computed over the FULL issue set,
    /// independent of any view filter, so a marker is not lost when the
    /// blocking issue falls outside the current view (e.g. a proposal
    /// blocking a todo). See SPEC r[blocker-indicators].
    pub fn blocker_status(&self) -> Result<HashMap<Uuid, BlockerStatus>> {
        // Serve from the invocation's cached graph when present, so a
        // list/next invocation that already loaded the set reads it once
        // (r[187 graph layer]).
        let (graph, _) = self.graph_or_load()?;
        let all_issues: Vec<Issue> = graph.issues().cloned().collect();
        Ok(build_blocker_status(&all_issues))
    }

    /// Load the full issue set once and keep it as the invocation's graph.
    /// Exactly one `storage.load_all_issues` (which itself reads the
    /// index); later `get_related_titles` / `get_incoming_relationships`
    /// calls in the same invocation are served from the cached graph, so a
    /// display command makes one read of the issue set per invocation.
    pub fn load_graph(&self) -> Result<(ProjectGraph, Vec<LoadWarning>)> {
        let (all_issues, warnings) = self.storage.load_all_issues()?;
        let graph = build_project_graph(&all_issues);
        let snapshot = (graph, warnings);
        let result = snapshot.clone();
        *self.graph.borrow_mut() = Some(snapshot);
        Ok(result)
    }

    /// The invocation's graph, loading it once when no command has loaded
    /// it yet.
    fn graph_or_load(&self) -> Result<(ProjectGraph, Vec<LoadWarning>)> {
        if let Some(snapshot) = self.graph.borrow().as_ref() {
            return Ok(snapshot.clone());
        }
        self.load_graph()
    }

    pub fn get_related_titles(&self, issue: &Issue) -> HashMap<Uuid, String> {
        self.graph_or_load()
            .map(|(graph, _)| graph.related_titles(issue))
            .unwrap_or_default()
    }

    /// Returns incoming relationships: other issues that have relationships pointing to this issue.
    /// Returns a Vec of (source_issue_id, source_issue_title, relation_type)
    /// plus the load warnings produced while reading the issue set
    /// (r[load-warnings-as-data]).
    pub fn get_incoming_relationships(
        &self,
        issue_id: &Uuid,
    ) -> Result<(Vec<(Uuid, String, RelationType)>, Vec<LoadWarning>)> {
        let (graph, warnings) = self.graph_or_load()?;
        Ok((graph.incoming_relationships(issue_id), warnings))
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

    /// Import the issues from a pebbles export JSON bundle (`pebbles
    /// import`, r[import-semantics]): parse and validate the bundle with the
    /// pure functions from `export` (every issue must pass the same schema
    /// checks as a loaded issue file, duplicate bundle ids are rejected,
    /// and an id that already exists in this store is an error), drop
    /// relationships whose target is neither in the bundle nor in the store
    /// (with a warning), then write every issue and commit the store change
    /// in ONE jj commit. Additive only: existing issues are never
    /// overwritten. Returns the number of issues imported and the warnings
    /// (per-issue schema warnings + dropped-relationship warnings) as data
    /// (r[load-warnings-as-data]); `source` names the bundle file in the
    /// commit message.
    pub fn import_bundle(&self, content: &str, source: &str) -> Result<MutationOutcome<usize>> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        // Validation before any write: the bundle parses, every issue
        // passes the loaded-issue-file schema checks, and no id collides
        // with the store or repeats inside the bundle.
        let plan = parse_export_bundle(content)?;
        let existing = self.storage.load_index()?;
        validate_import(&plan.issues, &existing.all_ids())?;

        // Relationships may only target an issue that exists after the
        // import: the union of the bundle's ids and the store's ids.
        let mut known = HashSet::new();
        for id in existing.all_ids() {
            known.insert(id);
        }
        for issue in &plan.issues {
            known.insert(issue.id);
        }
        let (issues, mut warnings) = drop_missing_relationships(&plan.issues, &known);
        for warning in &plan.warnings {
            warnings.push(warning.clone());
        }

        // Write every issue, then the index, then commit the whole
        // operation in one jj commit (r[jj-commit-complete-path-set]).
        let mut index = self.storage.load_index()?;
        let mut commit_paths: Vec<PathBuf> = Vec::new();
        let mut own_files: Vec<String> = Vec::new();
        for issue in &issues {
            let outcome = self.storage.save_issue(issue, &mut index)?;
            commit_paths.push(self.storage.issue_path(&outcome.new_file_name));
            own_files.push(outcome.new_file_name.clone());
        }
        let own_refs: Vec<&str> = own_files.iter().map(String::as_str).collect();
        self.storage
            .save_index_stale_aware(&index, &own_refs, &mut warnings)?;
        commit_paths.push(self.storage.index_path().clone());

        let path_refs: Vec<&Path> = commit_paths.iter().map(|p| p.as_path()).collect();
        self.commit_store_fallible(
            &format!("Import {} issue(s) from {}", issues.len(), source),
            &path_refs,
            &mut warnings,
        )?;

        Ok(MutationOutcome {
            value: issues.len(),
            warnings,
        })
    }

    /// Rebuild `.pebbles/index.json` from the issue files: the sanctioned
    /// repair for a conflicted or desynchronized index (r[reindex]). The
    /// rebuilt index stays in the working copy; the caller commits it when
    /// appropriate (this command deliberately never runs jj - the typical
    /// repair context is a conflicted index after a merge, which jj commit
    /// cannot meaningfully handle).
    /// Detect divergence between the store's index and its issue files
    /// (SPEC r[index-consistent]). Report only; the repair half is
    /// `pebbles reindex` (r[reindex]). Follows the 187 single-load rule:
    /// the issue set is read at most once per invocation (one pass per
    /// indexed file; orphan files are only listed, never parsed).
    pub fn doctor(&self) -> Result<DoctorOutcome> {
        if !self.storage.exists() {
            bail!("Pebbles not initialized. Run 'pebbles init' first.");
        }

        let index = match self.storage.load_index() {
            Ok(index) => index,
            // The index does not parse (malformed JSON or an entry whose
            // id is not a UUID): no entry can be checked against disk.
            // Report the store-level unknown and stop; `pebbles reindex`
            // can rebuild the index from the issue files.
            Err(e) => {
                return Ok(DoctorOutcome {
                    total_issues: 0,
                    diagnostics: vec![Diagnostic {
                        class: DiagnosticClass::UnknownIndexEntry,
                        id: None,
                        file_name: format!("{}", "index.json"),
                        detail: format!(
                            "(the index does not parse: {}; run 'pebbles reindex' to rebuild)",
                            e
                        ),
                    }],
                })
            }
        };

        // One read per indexed issue file: a file that parses records the
        // id it declares, a file that exists but does not parse records
        // `None`, and a file absent from disk records nothing (it surfaces
        // as missing below).
        let mut loads: Vec<IndexedFileLoad> = Vec::new();
        for metadata in &index.issues {
            let path = self.storage.issue_path(&metadata.file_name);
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Issue>(&content) {
                    Ok(issue) => loads.push(IndexedFileLoad {
                        file_name: metadata.file_name.clone(),
                        id: Some(issue.id),
                    }),
                    Err(_) => loads.push(IndexedFileLoad {
                        file_name: metadata.file_name.clone(),
                        id: None,
                    }),
                },
                Err(_) => {}
            }
        }

        // Issue file names on disk, sorted so the pure classification
        // input - and therefore the diagnostics - is deterministic.
        let entries = fs::read_dir(&self.storage.issue_path(""))
            .context("Failed to list .pebbles/issues directory")?;
        let mut disk_files: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.context("Failed to read .pebbles/issues directory entry")?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") {
                disk_files.push(name);
            }
        }
        disk_files.sort();
        let disk_refs: Vec<&str> = disk_files.iter().map(|f| f.as_str()).collect();

        let diagnostics = doctor(&index, &disk_refs, &loads);
        Ok(DoctorOutcome {
            total_issues: index.issues.len(),
            diagnostics,
        })
    }

    pub fn reindex(&self) -> Result<ReindexOutcome> {
        self.storage.reindex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Index, IssueMetadata, Relationship};

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

    // -- project graph --------------------------------------------------------

    #[test]
    fn build_project_graph_serves_related_titles_and_incoming() {
        let mut a = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Alpha",
            Status::Todo,
            IssueType::Bug,
        );
        let mut b = issue_with_type(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "Beta",
            Status::Todo,
            IssueType::Bug,
        );
        a.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: b.id,
        });
        b.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: a.id,
        });

        let graph = build_project_graph(&[a.clone(), b.clone()]);

        // related-titles lookup resolves targets from the in-memory set
        assert_eq!(
            graph.related_titles(&a),
            HashMap::from([(b.id, "Beta".to_string())])
        );

        // incoming lookup, source_title carried the same way as
        // build_incoming_relationships_map
        assert_eq!(
            graph.incoming_relationships(&b.id),
            vec![(a.id, "Alpha".to_string(), RelationType::PartOf)]
        );
        assert_eq!(
            graph.incoming_relationships(&a.id),
            vec![(b.id, "Beta".to_string(), RelationType::RequiredFor)]
        );
    }

    #[test]
    fn build_project_graph_counts_epic_children() {
        let mut epic = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Epic",
            Status::Todo,
            IssueType::Epic,
        );
        let mut child_done = issue_with_type(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "Child done",
            Status::Done,
            IssueType::Bug,
        );
        let mut child_todo = issue_with_type(
            "cccccccc-1111-2222-3333-444444444444",
            "Child todo",
            Status::Todo,
            IssueType::Bug,
        );
        child_done.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic.id,
        });
        child_todo.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic.id,
        });

        let graph = build_project_graph(&[child_done.clone(), child_todo.clone(), epic.clone()]);
        assert_eq!(graph.epic_counts().get(&epic.id), Some(&(2, 1)));
    }

    #[test]
    fn graph_serves_convert_epic_child_guard() {
        // The convert guard "cannot convert an epic with children" runs
        // the same predicate over the graph's issue set: any issue carries
        // a part_of edge targeting the epic.
        let epic = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Epic",
            Status::Todo,
            IssueType::Epic,
        );
        let mut child = issue_with_type(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "Child",
            Status::Todo,
            IssueType::Bug,
        );
        let unrelated = issue_with_type(
            "cccccccc-1111-2222-3333-444444444444",
            "Unrelated",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut child, RelationType::PartOf, &epic);

        let graph = build_project_graph(&[epic.clone(), child.clone(), unrelated.clone()]);

        let has_children = graph.issues().any(|i| {
            i.relationships
                .iter()
                .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == epic.id)
        });
        assert!(has_children);

        // An epic without children has no such edge.
        let childless_graph = build_project_graph(&[epic.clone(), unrelated.clone()]);
        let has_children = childless_graph.issues().any(|i| {
            i.relationships
                .iter()
                .any(|rel| rel.relation_type == RelationType::PartOf && rel.target_id == epic.id)
        });
        assert!(!has_children);

        // The other convert guard: the target issue itself is part of an
        // epic (an issue can be converted to an epic only when it has no
        // part_of relationship). The graph serves the issue by id exactly
        // as load_issue would.
        let from_graph = graph.issues().find(|i| i.id == child.id).unwrap();
        assert_eq!(from_graph.title, "Child");
        assert!(from_graph
            .relationships
            .iter()
            .any(|rel| rel.relation_type == RelationType::PartOf));
    }

    #[test]
    fn graph_serves_link_lookup_and_dag_check() {
        // link's query phase resolves source/target titles and builds the
        // dependency adjacency from the graph. The graph iterates a
        // HashMap, so the adjacency must be order-insensitive: shuffle the
        // input and compare against the store-order slice.
        let mut a = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "A",
            Status::Todo,
            IssueType::Bug,
        );
        let mut b = issue_with_type(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "B",
            Status::Todo,
            IssueType::Bug,
        );
        let c = issue_with_type(
            "cccccccc-1111-2222-3333-444444444444",
            "C",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut a, RelationType::RequiredFor, &b);
        relate(&mut b, RelationType::RequiredFor, &c);

        let in_order = vec![a.clone(), b.clone(), c.clone()];
        let graph = build_project_graph(&in_order);
        let from_graph: Vec<Issue> = graph.issues().cloned().collect();

        let adjacency = build_dependency_adjacency(&from_graph);
        assert_eq!(adjacency, build_dependency_adjacency(&in_order));

        // Adding the edge B -> A would close the cycle A -> B -> C -> A;
        // the graph-served adjacency rejects it exactly like the store
        // slice does. Adding A -> C closes nothing (C has no outgoing
        // edges), so it is accepted.
        assert!(creates_cycle(&adjacency, b.id, a.id));
        assert!(creates_cycle(&adjacency, c.id, a.id));
        assert!(!creates_cycle(&adjacency, a.id, c.id));

        // Source/target lookup by id, as link resolves its two endpoints.
        assert_eq!(graph.issues().find(|i| i.id == b.id).unwrap().title, "B");
        assert_eq!(graph.issues().find(|i| i.id == c.id).unwrap().title, "C");
    }

    #[test]
    fn graph_serves_delete_referencing_search() {
        // delete's query phase looks the target up by id and collects every
        // issue with a relationship pointing at it from the graph (the old
        // second load_all_issues). The graph iterates a HashMap, so the
        // referencing set is compared order-insensitively; ordering for the
        // guarded-delete listing comes from the index at call time.
        let mut target = issue_with_type(
            "aaaaaaaa-1111-2222-3333-444444444444",
            "Target",
            Status::Todo,
            IssueType::Bug,
        );
        let mut source = issue_with_type(
            "bbbbbbbb-1111-2222-3333-444444444444",
            "Source",
            Status::Todo,
            IssueType::Bug,
        );
        let mut other = issue_with_type(
            "cccccccc-1111-2222-3333-444444444444",
            "Other",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut source, RelationType::RequiredFor, &target);
        relate(&mut other, RelationType::PartOf, &target);

        let graph = build_project_graph(&[target.clone(), source.clone(), other.clone()]);

        // The graph serves the target by id exactly as load_issue would.
        assert_eq!(
            graph.issues().find(|i| i.id == target.id).unwrap().title,
            "Target"
        );

        // Every issue with a relationship pointing at the target surfaces,
        // and issues pointing elsewhere never do: the predicate delete's
        // broken-link guards run over the graph.
        let referencing: Vec<Issue> = graph
            .issues()
            .filter(|i| i.relationships.iter().any(|rel| rel.target_id == target.id))
            .cloned()
            .collect();
        let mut got = ids(&referencing);
        got.sort();
        let mut want = vec![source.id, other.id];
        want.sort();
        assert_eq!(got, want);
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

    fn epic_with(id: &str, title: &str, status: Status) -> Issue {
        let mut issue = Issue::new(title.to_string(), None, IssueType::Epic);
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

    #[test]
    fn get_unblocked_issues_epic_with_unblocked_child_is_hidden() {
        let epic = epic_with("11111111-2222-3333-4444-555555555555", "epic", Status::Todo);
        let mut child = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "child",
            Status::Todo,
        );
        relate(&mut child, RelationType::PartOf, &epic);
        let out = get_unblocked_issues(&[epic, child.clone()]);
        assert_eq!(ids(&out), vec![child.id]);
    }

    #[test]
    fn get_unblocked_issues_epic_with_only_blocked_children_stays() {
        // The child is blocked by its dependency, so the epic still
        // represents the work in next.
        let epic = epic_with("11111111-2222-3333-4444-555555555555", "epic", Status::Todo);
        let mut blocker = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "blocker",
            Status::Todo,
        );
        let mut child = issue_with(
            "33333333-2222-3333-4444-555555555555",
            "child",
            Status::Todo,
        );
        relate(&mut child, RelationType::PartOf, &epic);
        relate(&mut blocker, RelationType::RequiredFor, &child);
        let mut sorted = vec![epic.clone(), blocker.clone()];
        sorted.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));
        let out = get_unblocked_issues(&[epic, blocker, child]);
        assert_eq!(ids(&out), ids(&sorted));
    }

    #[test]
    fn get_unblocked_issues_epic_with_no_children_stays() {
        let epic = epic_with("11111111-2222-3333-4444-555555555555", "epic", Status::Todo);
        let out = get_unblocked_issues(&[epic.clone()]);
        assert_eq!(ids(&out), vec![epic.id]);
    }

    #[test]
    fn get_unblocked_issues_nested_epics_hide_up_to_innermost_unblocked() {
        // epic A contains epic B contains child C; with all three
        // unblocked, A is represented by B and B by C, so only C
        // surfaces in next.
        let a = epic_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = epic_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let mut c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        relate(&mut b, RelationType::PartOf, &a);
        relate(&mut c, RelationType::PartOf, &b);
        let out = get_unblocked_issues(&[a, b, c.clone()]);
        assert_eq!(ids(&out), vec![c.id]);
    }

    // -- build_blocker_status -------------------------------------------------

    #[test]
    fn build_blocker_status_ready_when_no_incomplete_dependencies() {
        let done_dep = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Done dep",
            Status::Done,
        );
        let mut ready = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Ready",
            Status::Todo,
        );
        relate(&mut ready, RelationType::RequiredFor, &done_dep);
        let map = build_blocker_status(&[done_dep, ready.clone()]);
        let status = &map[&ready.id];
        assert!(status.ready);
        assert!(status.blocked_by.is_empty());
        assert!(status.blocking.is_empty());
    }

    #[test]
    fn build_blocker_status_blocked_by_incomplete_dependency() {
        let mut blocker = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Todo blocker",
            Status::Todo,
        );
        let blocked = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Blocked",
            Status::Todo,
        );
        relate(&mut blocker, RelationType::RequiredFor, &blocked);
        let map = build_blocker_status(&[blocker.clone(), blocked.clone()]);
        // The blocker is a prerequisite the blocked issue depends on, so the
        // blocked issue lists it in blocked_by and is not ready.
        let status = &map[&blocked.id];
        assert!(!status.ready);
        assert_eq!(status.blocked_by, vec![blocker.id]);
        assert!(status.blocking.is_empty());
        // The blocker is shown as blocking its dependent.
        let blocker_status = &map[&blocker.id];
        assert!(blocker_status.ready);
        assert_eq!(blocker_status.blocking, vec![blocked.id]);
    }

    #[test]
    fn build_blocker_status_unique_ids_in_display_order() {
        let mut blocker1 = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "blocker1",
            Status::Todo,
        );
        let mut blocker2 = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "blocker2",
            Status::InProgress,
        );
        let blocked = issue_with(
            "33333333-2222-3333-4444-555555555555",
            "Blocked",
            Status::Todo,
        );
        relate(&mut blocker1, RelationType::RequiredFor, &blocked);
        relate(&mut blocker2, RelationType::RequiredFor, &blocked);
        relate(&mut blocker1, RelationType::RequiredFor, &blocked); // duplicate edge
        let map = build_blocker_status(&[blocker1.clone(), blocker2.clone(), blocked.clone()]);
        let blocked_status = &map[&blocked.id];
        assert_eq!(blocked_status.blocked_by, vec![blocker1.id, blocker2.id]);
        // Each blocker lists the single dependent once (dup edge deduped).
        let status1 = &map[&blocker1.id];
        assert_eq!(status1.blocking, vec![blocked.id]);
        let status2 = &map[&blocker2.id];
        assert_eq!(status2.blocking, vec![blocked.id]);
    }

    #[test]
    fn build_blocker_status_done_dependency_does_not_block() {
        let mut dep = issue_with("11111111-2222-3333-4444-555555555555", "dep", Status::Done);
        let target = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "target",
            Status::Todo,
        );
        relate(&mut dep, RelationType::RequiredFor, &target);
        let map = build_blocker_status(&[dep.clone(), target.clone()]);
        // The dependency is Done, so the dependent is not blocked. The done
        // dependency is itself not blocked and blocks nothing.
        assert!(map[&target.id].ready);
        assert!(map[&target.id].blocked_by.is_empty());
        assert!(map[&dep.id].ready);
        assert!(map[&dep.id].blocked_by.is_empty());
        assert!(map[&dep.id].blocking.is_empty());
    }

    #[test]
    fn build_blocker_status_ready_issue_free_of_relationships_is_ready() {
        let standalone = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Standalone",
            Status::Todo,
        );
        let map = build_blocker_status(&[standalone.clone()]);
        assert!(map[&standalone.id].ready);
        assert!(map[&standalone.id].blocked_by.is_empty());
        assert!(map[&standalone.id].blocking.is_empty());
    }

    #[test]
    fn build_blocker_status_blocking_lists_dependents() {
        let mut fuzzy = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Fuzzy search",
            Status::Todo,
        );
        let bulk = issue_with(
            "33333333-2222-3333-4444-555555555555",
            "Bulk operations",
            Status::Todo,
        );
        relate(&mut fuzzy, RelationType::RequiredFor, &bulk);
        let map = build_blocker_status(&[fuzzy.clone(), bulk.clone()]);
        // Fuzzy search is required_for bulk operations: bulk cannot start
        // until fuzzy is done, so bulk is blocked by fuzzy, and fuzzy is
        // shown as blocking bulk.
        let bulk_status = &map[&bulk.id];
        assert!(!bulk_status.ready);
        assert_eq!(bulk_status.blocked_by, vec![fuzzy.id]);
        assert!(bulk_status.blocking.is_empty());
        let fuzzy_status = &map[&fuzzy.id];
        assert!(fuzzy_status.ready);
        assert!(fuzzy_status.blocked_by.is_empty());
        assert_eq!(fuzzy_status.blocking, vec![bulk.id]);
    }

    #[test]
    fn build_blocker_status_done_dependent_not_listed_as_blocking() {
        // A required_for B; B is Done so A no longer blocks anything.
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Done);
        relate(&mut a, RelationType::RequiredFor, &b);
        let map = build_blocker_status(&[a.clone(), b.clone()]);
        // The dependent B is Done, so A blocks nothing (B no longer waits).
        assert!(map[&a.id].blocking.is_empty());
        assert!(map[&a.id].ready);
        assert!(map[&a.id].blocked_by.is_empty());
        // B is a Done issue that depends on the incomplete A, so it is not
        // ready (it still carries an incomplete required_for dependency).
        assert!(!map[&b.id].ready);
        assert_eq!(map[&b.id].blocked_by, vec![a.id]);
    }

    #[test]
    fn build_blocker_status_part_of_does_not_block() {
        let epic = issue_with_type(
            "11111111-2222-3333-4444-555555555555",
            "Epic",
            Status::Todo,
            IssueType::Epic,
        );
        let mut child = issue_with_type(
            "22222222-2222-3333-4444-555555555555",
            "Child",
            Status::Todo,
            IssueType::Bug,
        );
        relate(&mut child, RelationType::PartOf, &epic);
        let map = build_blocker_status(&[epic.clone(), child.clone()]);
        assert!(map[&epic.id].ready);
        assert!(map[&epic.id].blocking.is_empty());
        assert!(map[&child.id].ready);
        assert!(map[&child.id].blocked_by.is_empty());
    }

    // -- build_circular_issues ------------------------------------------------

    #[test]
    fn build_circular_issues_dag_has_no_circle() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        relate(&mut a, RelationType::RequiredFor, &b);
        assert!(build_circular_issues(&[a.clone(), b.clone()]).is_empty());
    }

    #[test]
    fn build_circular_issues_detects_two_node_cycle() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        relate(&mut a, RelationType::RequiredFor, &b);
        relate(&mut b, RelationType::RequiredFor, &a);
        let circular = build_circular_issues(&[a.clone(), b.clone()]);
        assert!(circular.contains(&a.id), "{circular:?}");
        assert!(circular.contains(&b.id), "{circular:?}");
    }

    #[test]
    fn build_circular_issues_detects_self_loop() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let a_id = a.id;
        // `pebbles link` rejects a self-edge; push one directly to model
        // hand-edited data.
        a.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: a_id,
        });
        let circular = build_circular_issues(&[a]);
        assert_eq!(circular.len(), 1);
        assert!(circular.contains(&a_id));
    }

    #[test]
    fn build_circular_issues_ignores_scheduled_before_hint_edges_by_default_dag() {
        // scheduled_before participates in the dependency graph for cycle
        // purposes (same adjacency as the flow layout) but never marks a
        // plain DAG circular.
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        relate(&mut a, RelationType::ScheduledBefore, &b);
        assert!(build_circular_issues(&[a, b]).is_empty());
    }

    #[test]
    fn build_blocker_status_marks_circular_members() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        relate(&mut a, RelationType::RequiredFor, &b);
        relate(&mut b, RelationType::RequiredFor, &a);
        let map = build_blocker_status(&[a.clone(), b.clone(), c.clone()]);
        assert!(map[&a.id].circular);
        assert!(map[&b.id].circular);
        assert!(!map[&c.id].circular);
        // A DAG keeps all flags clear.
        let mut d = issue_with("44444444-2222-3333-4444-555555555555", "D", Status::Todo);
        relate(&mut d, RelationType::RequiredFor, &c);
        let map = build_blocker_status(&[d.clone(), c.clone()]);
        assert!(!map[&d.id].circular);
        assert!(!map[&c.id].circular);
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
    // -- tag helpers ---------------------------------------------------------

    #[test]
    fn add_tags_appends_new_tags_in_input_order() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string(), "beta".to_string()];
        let added = add_tags_to_issue(&mut issue, &["gamma".to_string(), "delta".to_string()]);
        assert_eq!(
            added,
            ["gamma", "delta"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            issue.tags,
            ["alpha", "beta", "gamma", "delta"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn add_tags_dedupes_existing_and_within_input() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string()];
        // "alpha" already present, "gamma" repeated inside the input.
        let added = add_tags_to_issue(
            &mut issue,
            &[
                "alpha".to_string(),
                "gamma".to_string(),
                "gamma".to_string(),
            ],
        );
        assert_eq!(added, vec!["gamma".to_string()]);
        assert_eq!(
            issue.tags,
            ["alpha", "gamma"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn add_tags_no_change_when_all_tags_present() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string()];
        assert!(add_tags_to_issue(&mut issue, &["alpha".to_string()]).is_empty());
        assert_eq!(issue.tags, vec!["alpha".to_string()]);
    }

    // -- description append helpers -----------------------------------------

    #[test]
    fn append_description_text_rules() {
        assert_eq!(append_description_text(None, "line"), "line");
        assert_eq!(append_description_text(Some(""), "line"), "line");
        assert_eq!(append_description_text(Some("a"), "b"), "a\nb");
        assert_eq!(append_description_text(Some("a\n"), "b"), "a\nb");
        assert_eq!(append_description_text(Some("a"), ""), "a");
        assert_eq!(append_description_text(None, ""), "");
    }

    #[test]
    fn add_tags_is_case_sensitive() {
        // Tags are stored exactly as given; "Alpha" is a distinct tag from
        // "alpha".
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string()];
        let added = add_tags_to_issue(&mut issue, &["Alpha".to_string()]);
        assert_eq!(added, vec!["Alpha".to_string()]);
        assert_eq!(
            issue.tags,
            ["alpha", "Alpha"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn remove_tags_deletes_matching_and_keeps_others() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let removed = remove_tags_from_issue(&mut issue, &["beta".to_string(), "nope".to_string()]);
        assert_eq!(removed, vec!["beta".to_string()]);
        assert_eq!(
            issue.tags,
            ["alpha", "gamma"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn remove_tags_no_change_when_none_present() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string()];
        assert!(remove_tags_from_issue(&mut issue, &["nope".to_string()]).is_empty());
        assert_eq!(issue.tags, vec!["alpha".to_string()]);
    }

    #[test]
    fn remove_tags_is_case_sensitive() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        issue.tags = vec!["alpha".to_string()];
        assert!(remove_tags_from_issue(&mut issue, &["Alpha".to_string()]).is_empty());
        assert_eq!(issue.tags, vec!["alpha".to_string()]);
    }

    #[test]
    fn filter_by_tags_keeps_issues_carrying_every_filter_tag() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let mut c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        a.tags = vec!["x".to_string(), "y".to_string()];
        b.tags = vec!["x".to_string()];
        c.tags = vec!["y".to_string()];
        // AND semantics: only A carries both tags.
        let out = filter_by_tags(&[a.clone(), b, c], &["x".to_string(), "y".to_string()]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_tags_single_tag_and_order_preserved() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        a.tags = vec!["x".to_string()];
        b.tags = vec!["x".to_string()];
        // Input order is preserved (the caller sorts by updated_at first).
        let out = filter_by_tags(&[a.clone(), c, b.clone()], &["x".to_string()]);
        assert_eq!(ids(&out), vec![a.id, b.id]);
    }

    #[test]
    fn filter_by_tags_is_case_sensitive() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        a.tags = vec!["backend".to_string()];
        assert!(filter_by_tags(&[a.clone()], &["Backend".to_string()]).is_empty());
        assert_eq!(
            ids(&filter_by_tags(&[a.clone()], &["backend".to_string()])),
            vec![a.id]
        );
    }

    #[test]
    fn filter_by_tags_empty_filter_returns_everything() {
        // The CLI rejects an empty --tag filter before this is reachable;
        // vacuously, an issue carries every tag in an empty filter.
        let a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let out = filter_by_tags(&[a.clone()], &[]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    // -- metadata helpers ----------------------------------------------------

    #[test]
    fn set_metadata_inserts_new_key() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        assert!(set_metadata_on_issue(&mut issue, "priority", "high"));
        assert_eq!(
            issue.metadata.get("priority").map(String::as_str),
            Some("high")
        );
    }

    #[test]
    fn set_metadata_replaces_existing_value() {
        // Upsert: setting an existing key replaces its value in place.
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        set_metadata_on_issue(&mut issue, "priority", "high");
        assert!(set_metadata_on_issue(&mut issue, "priority", "low"));
        assert_eq!(
            issue.metadata.get("priority").map(String::as_str),
            Some("low")
        );
        assert_eq!(issue.metadata.len(), 1);
    }

    #[test]
    fn set_metadata_same_value_is_no_change() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        assert!(set_metadata_on_issue(&mut issue, "priority", "high"));
        // Setting the identical value again changes nothing (no write, no commit).
        assert!(!set_metadata_on_issue(&mut issue, "priority", "high"));
        assert_eq!(
            issue.metadata.get("priority").map(String::as_str),
            Some("high")
        );
    }

    #[test]
    fn set_metadata_accepts_empty_value() {
        // Values are free-form strings: an empty value is a valid presence
        // marker (r[metadata-set-upsert]).
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        assert!(set_metadata_on_issue(&mut issue, "triage", ""));
        assert_eq!(issue.metadata.get("triage").map(String::as_str), Some(""));
    }

    #[test]
    fn unset_metadata_removes_key() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        set_metadata_on_issue(&mut issue, "priority", "high");
        assert!(unset_metadata_on_issue(&mut issue, "priority"));
        assert!(issue.metadata.is_empty());
    }

    #[test]
    fn unset_metadata_absent_key_is_no_op() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        assert!(!unset_metadata_on_issue(&mut issue, "priority"));
        assert!(issue.metadata.is_empty());
    }

    #[test]
    fn unset_metadata_is_case_sensitive() {
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        set_metadata_on_issue(&mut issue, "priority", "high");
        assert!(!unset_metadata_on_issue(&mut issue, "Priority"));
        assert_eq!(
            issue.metadata.get("priority").map(String::as_str),
            Some("high")
        );
    }

    #[test]
    fn filter_by_metadata_keeps_issues_with_every_filter_pair() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let mut c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        set_metadata_on_issue(&mut a, "priority", "high");
        set_metadata_on_issue(&mut a, "team", "backend");
        set_metadata_on_issue(&mut b, "priority", "high");
        set_metadata_on_issue(&mut c, "team", "backend");
        // AND semantics: only A carries both pairs (r[metadata-filter-and]).
        let out = filter_by_metadata(
            &[a.clone(), b, c],
            &[
                ("priority".to_string(), "high".to_string()),
                ("team".to_string(), "backend".to_string()),
            ],
        );
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_metadata_exact_match_on_key_and_value() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        let c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        set_metadata_on_issue(&mut a, "priority", "high");
        set_metadata_on_issue(&mut b, "priority", "low"); // key present, value differs
                                                          // c has no metadata at all.
        let out = filter_by_metadata(
            &[a.clone(), b, c],
            &[("priority".to_string(), "high".to_string())],
        );
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_metadata_is_case_sensitive() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        set_metadata_on_issue(&mut a, "priority", "high");
        assert!(filter_by_metadata(
            &[a.clone()],
            &[("Priority".to_string(), "high".to_string())]
        )
        .is_empty());
        assert!(filter_by_metadata(
            &[a.clone()],
            &[("priority".to_string(), "High".to_string())]
        )
        .is_empty());
        assert_eq!(
            ids(&filter_by_metadata(
                &[a.clone()],
                &[("priority".to_string(), "high".to_string())]
            )),
            vec![a.id]
        );
    }

    #[test]
    fn filter_by_metadata_matches_empty_value() {
        // A pair with an empty value matches metadata that stores "" for that
        // key (and nothing else) - the filter value is matched exactly.
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        set_metadata_on_issue(&mut a, "triage", "");
        set_metadata_on_issue(&mut b, "triage", "no");
        let out = filter_by_metadata(&[a.clone(), b], &[("triage".to_string(), "".to_string())]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_metadata_empty_filter_returns_everything() {
        // The CLI rejects a filter with a missing '=' or empty key before
        // this is reachable; vacuously, an issue carries every pair in an
        // empty filter.
        let a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let out = filter_by_metadata(&[a.clone()], &[]);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_metadata_preserves_input_order() {
        let mut a = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        let c = issue_with("33333333-2222-3333-4444-555555555555", "C", Status::Todo);
        let mut b = issue_with("22222222-2222-3333-4444-555555555555", "B", Status::Todo);
        set_metadata_on_issue(&mut a, "priority", "high");
        set_metadata_on_issue(&mut b, "priority", "high");
        // Input order is preserved (the caller sorts by updated_at first).
        let out = filter_by_metadata(
            &[a.clone(), c, b.clone()],
            &[("priority".to_string(), "high".to_string())],
        );
        assert_eq!(ids(&out), vec![a.id, b.id]);
    }

    #[test]
    fn parse_metadata_filter_splits_on_first_equals() {
        // Values may contain '=': the pair is split at the FIRST '='.
        let pair = parse_metadata_filter("a=b=c").unwrap();
        assert_eq!(pair, ("a".to_string(), "b=c".to_string()));
        let pair = parse_metadata_filter("triage=").unwrap();
        assert_eq!(pair, ("triage".to_string(), "".to_string()));
    }

    #[test]
    fn parse_metadata_filter_trims_key_and_value() {
        let pair = parse_metadata_filter("  priority = high  ").unwrap();
        assert_eq!(pair, ("priority".to_string(), "high".to_string()));
    }

    #[test]
    fn parse_metadata_filter_rejects_missing_equals_or_empty_key() {
        // r[metadata-filter-parse]: a pair without '=' is not a selector, and
        // neither is a blank key (r[continuity]).
        assert!(parse_metadata_filter("priority").is_err());
        assert!(parse_metadata_filter("=high").is_err());
        assert!(parse_metadata_filter("  =high").is_err());
    }

    #[test]
    fn validate_metadata_key_rejects_empty_and_whitespace() {
        assert!(validate_metadata_key("priority").is_ok());
        assert!(validate_metadata_key("").is_err());
        assert!(validate_metadata_key("  ").is_err());
    }

    #[test]
    fn sorted_metadata_orders_pairs_by_key() {
        // HashMap iteration order is unspecified; display sorts for stable
        // output (r[metadata-get-sorted]).
        let mut issue = issue_with("11111111-2222-3333-4444-555555555555", "A", Status::Todo);
        set_metadata_on_issue(&mut issue, "zeta", "1");
        set_metadata_on_issue(&mut issue, "alpha", "2");
        set_metadata_on_issue(&mut issue, "mid", "3");
        assert_eq!(
            sorted_metadata(&issue),
            vec![
                ("alpha".to_string(), "2".to_string()),
                ("mid".to_string(), "3".to_string()),
                ("zeta".to_string(), "1".to_string()),
            ]
        );
    }

    // -- type templates (r[type-templates]) ------------------------------------

    #[test]
    fn template_fields_for_bug_prompts_expected_and_actual() {
        // Bug issues ask what should have happened and what did happen.
        let fields = template_fields_for(IssueType::Bug);
        assert_eq!(
            fields
                .iter()
                .map(|f| f.key.clone())
                .collect::<Vec<String>>(),
            vec!["expected".to_string(), "actual".to_string()]
        );
        assert_eq!(fields[0].label, "Expected behavior");
        assert_eq!(fields[1].label, "Actual behavior");
    }

    #[test]
    fn template_fields_for_ux_problem_prompts_before_and_after_screenshots() {
        // UX issues ask for reproduction evidence: before/after screenshots.
        let fields = template_fields_for(IssueType::UxProblem);
        assert_eq!(
            fields
                .iter()
                .map(|f| f.key.clone())
                .collect::<Vec<String>>(),
            vec![
                "before_screenshot".to_string(),
                "after_screenshot".to_string(),
            ]
        );
        assert_eq!(fields[0].label, "Before screenshot (path or URL)");
        assert_eq!(fields[1].label, "After screenshot (path or URL)");
    }

    #[test]
    fn template_fields_for_usecase_prompts_acceptance_criteria() {
        // Usecases ask for the checkable definition of done.
        let fields = template_fields_for(IssueType::Usecase);
        assert_eq!(
            fields
                .iter()
                .map(|f| f.key.clone())
                .collect::<Vec<String>>(),
            vec!["acceptance_criteria".to_string()]
        );
        assert_eq!(fields[0].label, "Acceptance criteria");
    }

    #[test]
    fn template_fields_for_types_without_templates_is_empty() {
        // Types without a template prompt nothing: the interactive create
        // flow then behaves exactly as before.
        for issue_type in [
            IssueType::Design,
            IssueType::Validation,
            IssueType::Epic,
            IssueType::Milestone,
        ] {
            assert!(template_fields_for(issue_type).is_empty());
        }
    }

    // -- parse_text_query / filter_by_text_query ------------------------------

    fn issue_with_description(id: &str, title: &str, description: &str, status: Status) -> Issue {
        let mut issue = issue_with(id, title, status);
        issue.description = Some(description.to_string());
        issue
    }

    #[test]
    fn parse_text_query_blank_query_has_no_terms() {
        assert_eq!(parse_text_query("").unwrap(), Vec::new());
        assert_eq!(parse_text_query("   ").unwrap(), Vec::new());
        assert_eq!(parse_text_query("\t ").unwrap(), Vec::new());
    }

    #[test]
    fn parse_text_query_single_term_is_substring() {
        assert_eq!(
            parse_text_query("login").unwrap(),
            vec![TextQueryTerm::Substring("login".to_string())]
        );
    }

    #[test]
    fn parse_text_query_terms_are_lowercased() {
        assert_eq!(
            parse_text_query("LOGIN").unwrap(),
            vec![TextQueryTerm::Substring("login".to_string())]
        );
    }

    #[test]
    fn parse_text_query_space_separated_terms_are_anded() {
        assert_eq!(
            parse_text_query("login mobile").unwrap(),
            vec![
                TextQueryTerm::Substring("login".to_string()),
                TextQueryTerm::Substring("mobile".to_string()),
            ]
        );
        // Runs of whitespace collapse; surrounding whitespace is trimmed.
        assert_eq!(
            parse_text_query("  login \t mobile  ").unwrap(),
            vec![
                TextQueryTerm::Substring("login".to_string()),
                TextQueryTerm::Substring("mobile".to_string()),
            ]
        );
    }

    #[test]
    fn parse_text_query_quoted_phrase_matches_verbatim() {
        assert_eq!(
            parse_text_query("\"login button\"").unwrap(),
            vec![TextQueryTerm::Phrase("login button".to_string())]
        );
        // Internal spacing is preserved inside a phrase.
        assert_eq!(
            parse_text_query("\"login  button\"").unwrap(),
            vec![TextQueryTerm::Phrase("login  button".to_string())]
        );
    }

    #[test]
    fn parse_text_query_mixes_terms_and_phrases() {
        assert_eq!(
            parse_text_query("login \"jwt token\" auth").unwrap(),
            vec![
                TextQueryTerm::Substring("login".to_string()),
                TextQueryTerm::Phrase("jwt token".to_string()),
                TextQueryTerm::Substring("auth".to_string()),
            ]
        );
    }

    #[test]
    fn parse_text_query_unterminated_quote_is_an_error() {
        assert!(parse_text_query("\"login").is_err());
        assert!(parse_text_query("login \"jwt").is_err());
    }

    #[test]
    fn parse_text_query_empty_phrase_is_an_error() {
        assert!(parse_text_query("\"\"").is_err());
        assert!(parse_text_query("login \"\" mobile").is_err());
    }

    #[test]
    fn filter_by_text_query_matches_title_substring() {
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
        let terms = parse_text_query("jwt").unwrap();
        let out = filter_by_text_query(&[login, jwt], &terms);
        assert_eq!(
            ids(&out),
            vec![uuid("22222222-2222-3333-4444-555555555555")]
        );
    }

    #[test]
    fn filter_by_text_query_matches_description() {
        let login = issue_with_description(
            "11111111-2222-3333-4444-555555555555",
            "Login button",
            "The button does not respond to clicks",
            Status::Todo,
        );
        let terms = parse_text_query("clicks").unwrap();
        let out = filter_by_text_query(&[login.clone()], &terms);
        assert_eq!(ids(&out), vec![login.id]);
    }

    #[test]
    fn filter_by_text_query_is_case_insensitive() {
        let csv = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Export users to CSV",
            Status::Done,
        );
        let terms = parse_text_query("csv").unwrap();
        assert_eq!(
            ids(&filter_by_text_query(&[csv.clone()], &terms)),
            vec![csv.id]
        );
        let terms = parse_text_query("EXPORT").unwrap();
        assert_eq!(
            ids(&filter_by_text_query(&[csv.clone()], &terms)),
            vec![csv.id]
        );
    }

    #[test]
    fn filter_by_text_query_issue_without_description_matches_title_only() {
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login button",
            Status::Todo,
        );
        let terms = parse_text_query("clicks").unwrap();
        assert!(filter_by_text_query(&[login.clone()], &terms).is_empty());
        let terms = parse_text_query("login").unwrap();
        assert_eq!(
            ids(&filter_by_text_query(&[login.clone()], &terms)),
            vec![login.id]
        );
    }

    #[test]
    fn filter_by_text_query_terms_are_anded() {
        let a = issue_with_description(
            "11111111-2222-3333-4444-555555555555",
            "Login button",
            "Mobile app",
            Status::Todo,
        );
        let b = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Mobile app",
            Status::Todo,
        );
        let terms = parse_text_query("login mobile").unwrap();
        let out = filter_by_text_query(&[a.clone(), b], &terms);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_text_query_phrase_matches_verbatim_not_terms() {
        // 'jwt based' with a single space is not a substring of 'JWT-based';
        // the phrase matches nothing while the ANDed terms both match.
        let mut jwt = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "JWT token auth",
            Status::Todo,
        );
        jwt.description = Some("Design and implement JWT-based authentication".to_string());
        let phrase = parse_text_query("\"jwt based\"").unwrap();
        assert!(filter_by_text_query(&[jwt.clone()], &phrase).is_empty());
        let terms = parse_text_query("jwt based").unwrap();
        assert_eq!(
            ids(&filter_by_text_query(&[jwt.clone()], &terms)),
            vec![jwt.id]
        );
    }

    #[test]
    fn filter_by_text_query_phrase_and_term_are_anded() {
        let a = issue_with_description(
            "11111111-2222-3333-4444-555555555555",
            "Login button",
            "JWT token system",
            Status::Todo,
        );
        let b = issue_with_description(
            "22222222-2222-3333-4444-555555555555",
            "Login page",
            "JWT token system",
            Status::Todo,
        );
        let terms = parse_text_query("button \"jwt token\"").unwrap();
        let out = filter_by_text_query(&[a.clone(), b], &terms);
        assert_eq!(ids(&out), vec![a.id]);
    }

    #[test]
    fn filter_by_text_query_blank_query_matches_nothing() {
        let login = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        let terms = parse_text_query("").unwrap();
        assert!(filter_by_text_query(&[login.clone()], &terms).is_empty());
        let terms = parse_text_query("   ").unwrap();
        assert!(filter_by_text_query(&[login.clone()], &terms).is_empty());
    }

    #[test]
    fn filter_by_text_query_preserves_input_order() {
        // No relevance ranking (unlike --search): matches keep the list's
        // updated_at-descending order (r[issue-ordering]).
        let mut newer = issue_with(
            "11111111-2222-3333-4444-555555555555",
            "Login bug",
            Status::Todo,
        );
        let mut older = issue_with(
            "22222222-2222-3333-4444-555555555555",
            "Login app",
            Status::Todo,
        );
        newer.updated_at = chrono::Utc::now();
        older.updated_at = chrono::Utc::now() - chrono::Duration::seconds(60);
        let terms = parse_text_query("login").unwrap();
        let out = filter_by_text_query(&[newer.clone(), older.clone()], &terms);
        assert_eq!(ids(&out), vec![newer.id, older.id]);
    }

    // -- select_creation_tips (r[guidance-tips]) -------------------------------

    /// A list of existing epics as (id, title) pairs, the shape
    /// `select_creation_tips` receives from the CLI.
    fn epics(epics: &[(&str, &str)]) -> Vec<(Uuid, String)> {
        epics
            .iter()
            .map(|(id, title)| (uuid(id), title.to_string()))
            .collect()
    }

    #[test]
    fn creation_guidance_bug_with_epics_suggests_epic_and_lists_candidates() {
        let guidance = select_creation_tips(
            &IssueType::Bug,
            &Vec::new(),
            &epics(&[
                ("aaaaaaaa-0000-0000-0000-000000000001", "User Auth"),
                ("bbbbbbbb-0000-0000-0000-000000000002", "Payments"),
            ]),
        );
        assert!(guidance.suggest_epic);
        assert_eq!(
            guidance
                .epic_candidates
                .iter()
                .map(|(_, title)| title.clone())
                .collect::<Vec<String>>(),
            vec!["User Auth", "Payments"]
        );
        // A fresh issue has no relationships: the dependency tip also applies
        // (both tips are optional and independent).
        assert!(guidance.suggest_dependency);
    }

    #[test]
    fn creation_guidance_without_epics_suggests_nothing() {
        let guidance =
            select_creation_tips(&IssueType::Bug, &Vec::new(), &Vec::<(Uuid, String)>::new());
        assert!(!guidance.suggest_epic);
        assert!(guidance.epic_candidates.is_empty());
        // An epic-less project gets no guidance at all: there is no epic to
        // link into, so both tips would be noise (r[guidance-tips]).
        assert!(!guidance.suggest_dependency);
    }

    #[test]
    fn creation_guidance_epic_creation_never_suggests_epic() {
        // An epic itself cannot be part_of another epic
        // (r[part-of-epic-types]): no epic tip and no candidate list.
        let guidance = select_creation_tips(
            &IssueType::Epic,
            &Vec::new(),
            &epics(&[("aaaaaaaa-0000-0000-0000-000000000001", "User Auth")]),
        );
        assert!(!guidance.suggest_epic);
        assert!(guidance.epic_candidates.is_empty());
        // The dependency tip is orthogonal to the epic tip: a fresh epic
        // still has no relationships.
        assert!(guidance.suggest_dependency);
    }

    #[test]
    fn creation_guidance_milestone_never_suggests_epic() {
        let guidance = select_creation_tips(
            &IssueType::Milestone,
            &Vec::new(),
            &epics(&[("aaaaaaaa-0000-0000-0000-000000000001", "User Auth")]),
        );
        assert!(!guidance.suggest_epic);
        assert!(guidance.epic_candidates.is_empty());
    }

    #[test]
    fn creation_guidance_existing_relationships_suppress_tips() {
        // Already part of an epic: re-suggesting the epic link would be noise.
        let mut rels: Vec<Relationship> = Vec::new();
        rels.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: uuid("aaaaaaaa-0000-0000-0000-000000000001"),
        });
        let guidance = select_creation_tips(
            &IssueType::Bug,
            &rels,
            &epics(&[("aaaaaaaa-0000-0000-0000-000000000001", "User Auth")]),
        );
        assert!(!guidance.suggest_epic);
        assert!(guidance.epic_candidates.is_empty());
        assert!(!guidance.suggest_dependency);
    }

    #[test]
    fn creation_guidance_dependency_tip_requires_no_relationships() {
        // Any existing relationship suppresses the dependency tip: there is
        // nothing new to teach.
        let mut rels: Vec<Relationship> = Vec::new();
        rels.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: uuid("bbbbbbbb-0000-0000-0000-000000000002"),
        });
        let guidance = select_creation_tips(
            &IssueType::Bug,
            &rels,
            &epics(&[("aaaaaaaa-0000-0000-0000-000000000001", "User Auth")]),
        );
        assert!(!guidance.suggest_dependency);
    }

    // -- pebbles doctor (r[index-consistent]) --------------------------------

    fn metadata(id: &str, file_name: &str) -> IssueMetadata {
        IssueMetadata {
            id: uuid(id),
            file_name: file_name.to_string(),
        }
    }

    fn index_with(entries: &[(&str, &str)]) -> Index {
        let issues: Vec<IssueMetadata> = entries
            .iter()
            .map(|(id, file_name)| metadata(*id, *file_name))
            .collect();
        Index { issues }
    }

    fn loads_from(pairs: &[(&str, Option<&str>)]) -> Vec<IndexedFileLoad> {
        pairs
            .iter()
            .map(|(file_name, id)| IndexedFileLoad {
                file_name: file_name.to_string(),
                id: id.map(|s| uuid(s)),
            })
            .collect::<Vec<_>>()
    }

    fn classes(diagnostics: &[Diagnostic]) -> Vec<DiagnosticClass> {
        diagnostics
            .iter()
            .map(|d| d.class.clone())
            .collect::<Vec<_>>()
    }

    #[test]
    fn doctor_consistent_store_reports_nothing() {
        let index = index_with(&[
            (
                "11111111-1111-1111-1111-111111111111",
                "bug-11111111-one.json",
            ),
            (
                "22222222-1111-1111-1111-111111111111",
                "bug-22222222-two.json",
            ),
        ]);
        let disk = &["bug-11111111-one.json", "bug-22222222-two.json"];
        let loads = loads_from(&[
            (
                "bug-11111111-one.json",
                Some("11111111-1111-1111-1111-111111111111"),
            ),
            (
                "bug-22222222-two.json",
                Some("22222222-1111-1111-1111-111111111111"),
            ),
        ]);
        assert_eq!(doctor(&index, disk, &loads), Vec::<Diagnostic>::new());
    }

    #[test]
    fn doctor_reports_index_entries_without_a_file_on_disk() {
        // One indexed file was deleted from disk; the load pass reads only
        // files that exist, so the missing entry has no load row at all.
        let index = index_with(&[
            (
                "11111111-1111-1111-1111-111111111111",
                "bug-11111111-one.json",
            ),
            (
                "22222222-1111-1111-1111-111111111111",
                "bug-22222222-gone.json",
            ),
        ]);
        let disk = &["bug-11111111-one.json"];
        let loads = loads_from(&[(
            "bug-11111111-one.json",
            Some("11111111-1111-1111-1111-111111111111"),
        )]);
        let diagnostics = doctor(&index, disk, &loads);
        assert_eq!(classes(&diagnostics), vec![DiagnosticClass::MissingFile]);
        assert_eq!(diagnostics[0].file_name, "bug-22222222-gone.json");
    }

    #[test]
    fn doctor_reports_files_on_disk_without_an_index_entry() {
        // The repo's historical test-pollution files land here: issue
        // files on disk the index knows nothing about.
        let index = index_with(&[(
            "11111111-1111-1111-1111-111111111111",
            "bug-11111111-one.json",
        )]);
        let disk = &["bug-11111111-one.json", "stray-test-pollution.json"];
        let loads = loads_from(&[(
            "bug-11111111-one.json",
            Some("11111111-1111-1111-1111-111111111111"),
        )]);
        let diagnostics = doctor(&index, disk, &loads);
        assert_eq!(classes(&diagnostics), vec![DiagnosticClass::OrphanedFile]);
        assert_eq!(diagnostics[0].file_name, "stray-test-pollution.json");
    }

    #[test]
    fn doctor_reports_index_entries_that_do_not_parse_as_issues() {
        // The indexed file exists but its content is not a valid Issue:
        // the entry resolves to nothing, mirroring the load-time warning
        // for unparsable files (r[schema-valid-issues]).
        let index = index_with(&[
            (
                "11111111-1111-1111-1111-111111111111",
                "bug-11111111-one.json",
            ),
            (
                "44444444-1111-1111-1111-111111111111",
                "broken-44444444-not-an-issue.json",
            ),
        ]);
        let disk = &["bug-11111111-one.json", "broken-44444444-not-an-issue.json"];
        let loads = loads_from(&[
            (
                "bug-11111111-one.json",
                Some("11111111-1111-1111-1111-111111111111"),
            ),
            ("broken-44444444-not-an-issue.json", None),
        ]);
        let diagnostics = doctor(&index, disk, &loads);
        assert_eq!(
            classes(&diagnostics),
            vec![DiagnosticClass::UnknownIndexEntry]
        );
        assert_eq!(
            diagnostics[0].file_name,
            "broken-44444444-not-an-issue.json"
        );
    }

    #[test]
    fn doctor_reports_index_file_names_that_mismatch_the_file_on_disk() {
        // The index maps 22222222 to bug-22222222-jwt.json, but that file
        // parses as an issue declaring a different id (e.g. another file
        // copied over the indexed name).
        let index = index_with(&[(
            "22222222-1111-1111-1111-111111111111",
            "bug-22222222-jwt.json",
        )]);
        let disk = &["bug-22222222-jwt.json"];
        let loads = loads_from(&[(
            "bug-22222222-jwt.json",
            Some("55555555-1111-1111-1111-111111111111"),
        )]);
        let diagnostics = doctor(&index, disk, &loads);
        assert_eq!(classes(&diagnostics), vec![DiagnosticClass::FileMismatch]);
        assert_eq!(diagnostics[0].file_name, "bug-22222222-jwt.json");
    }

    #[test]
    fn doctor_output_is_deterministic_sorted_by_file_name() {
        // Every class in one store, index order scrambled: diagnostics
        // come back sorted by file name, one per troubled entry/file.
        let index = index_with(&[
            (
                "22222222-1111-1111-1111-111111111111",
                "bug-22222222-jwt.json",
            ),
            (
                "33333333-1111-1111-1111-111111111111",
                "uc-33333333-gone.json",
            ),
            (
                "11111111-1111-1111-1111-111111111111",
                "bug-11111111-one.json",
            ),
        ]);
        let disk = &[
            "zz-stray.json",
            "broken-44444444-x.json",
            "bug-11111111-one.json",
            "bug-22222222-jwt.json",
        ];
        let loads = loads_from(&[
            (
                "bug-11111111-one.json",
                Some("11111111-1111-1111-1111-111111111111"),
            ),
            (
                "bug-22222222-jwt.json",
                Some("55555555-1111-1111-1111-111111111111"),
            ),
        ]);
        let diagnostics = doctor(&index, disk, &loads);
        assert_eq!(
            classes(&diagnostics),
            vec![
                DiagnosticClass::OrphanedFile,
                DiagnosticClass::FileMismatch,
                DiagnosticClass::MissingFile,
                DiagnosticClass::OrphanedFile,
            ]
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|d| d.file_name.clone())
                .collect::<Vec<_>>(),
            vec![
                "broken-44444444-x.json",
                "bug-22222222-jwt.json",
                "uc-33333333-gone.json",
                "zz-stray.json",
            ]
        );
    }
}
