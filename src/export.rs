use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use uuid::Uuid;

use crate::models::{Issue, IssueType, RelationType, Relationship, Status};
use crate::storage::{unknown_top_level_keys, LoadWarning};

/// The envelope marker of a pebbles export bundle (r[export-bundle]).
///
/// ```rust
/// # use pebbles::export::EXPORT_FORMAT;
/// assert_eq!(EXPORT_FORMAT, "pebbles-export");
/// ```
pub const EXPORT_FORMAT: &str = "pebbles-export";

/// The schema version of the export bundle format this pebbles writes and
/// imports (r[export-bundle]). Bump on any incompatible change to the
/// envelope or to what an Issue element may contain; `pebbles import`
/// rejects other versions.
pub const EXPORT_VERSION: u64 = 1;

/// The JSON bundle produced by `pebbles export json` and accepted by
/// `pebbles import`: a versioned envelope carrying one [`Issue`] per
/// element, with every field the store knows (id, title, description,
/// status, issue_type, tags, relationships, metadata, timestamps)
/// (r[export-bundle]). Issues are listed in ascending id order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    pub format: String,
    pub version: u64,
    pub issues: Vec<Issue>,
}

/// The result of validating a bundle's issues before any write
/// (r[import-semantics]): the parsed issues plus per-issue warnings that
/// mirror the store's load warnings (an issue with keys outside the schema
/// is imported with a warning, exactly as a loaded issue file warns).
#[derive(Debug)]
pub struct ImportPlan {
    pub issues: Vec<Issue>,
    pub warnings: Vec<LoadWarning>,
}

/// A copy of `issues` sorted by id ascending (string order of the UUIDs).
/// Exports use this order so the output is canonical and diff-friendly
/// regardless of the in-memory load order (r[export-order]).
fn sorted_by_id(issues: &[Issue]) -> Vec<Issue> {
    let mut out: Vec<Issue> = issues.to_vec();
    out.sort_by_key(|a| a.id.to_string());
    out
}

/// A copy of `issues` sorted by title ascending (string order). Markdown
/// export lists epics, children, and orphans this way so the document is
/// deterministic (r[export-order]).
fn sorted_by_title(issues: &[Issue]) -> Vec<Issue> {
    let mut out: Vec<Issue> = issues.to_vec();
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

/// Serialize `issues` as a pebbles export JSON bundle (r[export-bundle]):
/// a `{"format": "pebbles-export", "version": 1, "issues": [...]}` envelope
/// with the issues sorted by id (r[export-order]), one Issue object per
/// element preserving every field of the store's issue file schema. Pure:
/// no storage access.
///
/// ```rust
/// # use pebbles::export::export_json;
/// # use pebbles::models::{Issue, IssueType};
/// let json = export_json(&vec![Issue::new("A".to_string(), None, IssueType::Bug)]);
/// assert!(json.contains("\"format\": \"pebbles-export\""));
/// assert!(json.contains("\"version\": 1"));
/// ```
pub fn export_json(issues: &[Issue]) -> String {
    let bundle = ExportBundle {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        issues: sorted_by_id(issues),
    };
    let mut content = serde_json::to_string_pretty(&bundle).unwrap();
    content.push('\n');
    content
}

/// Encode one CSV field per RFC-4180: always double-quoted, embedded
/// double quotes doubled (""). This makes every field -- including the
/// JSON-encoded tags, relationships, and metadata -- unambiguous and
/// parseable by any RFC-4180 reader (r[csv-schema]).
///
/// ```rust
/// # use pebbles::export::csv_encode_field;
/// assert_eq!(csv_encode_field("plain"), "\"plain\"");
/// assert_eq!(csv_encode_field("a,b"), "\"a,b\"");
/// assert_eq!(csv_encode_field("say \"hi\""), "\"say \"\"hi\"\"\"");
/// ```
pub fn csv_encode_field(field: &str) -> String {
    let inner = field.replace('"', "\"\"");
    format!("\"{}\"", inner)
}

/// Serialize `issues` as the export CSV (r[csv-schema]): one header row
/// `id,title,description,status,issue_type,tags,relationships,metadata`
/// then one row per issue, sorted by id (r[export-order]). Every field is
/// RFC-4180 encoded (double-quoted, embedded quotes doubled). The delimiter
/// is the comma, and records end with LF (RFC-4180 specifies CRLF; LF is
/// accepted by every CSV consumer and keeps exports diff-friendly).
/// `description` is empty when the issue has none; `status` and
/// `issue_type` use their canonical spellings; tags, relationships, and
/// metadata are JSON-encoded so no information is lost. Pure: no storage
/// access.
///
/// ```rust
/// # use pebbles::export::export_csv;
/// # use pebbles::models::{Issue, IssueType};
/// let issues = vec![Issue::new("A".to_string(), None, IssueType::Bug)];
/// let csv = export_csv(&issues);
/// assert!(csv.starts_with("id,title,description,status,issue_type,tags,relationships,metadata\n"));
/// ```
pub fn export_csv(issues: &[Issue]) -> String {
    let mut lines: Vec<String> =
        vec!["id,title,description,status,issue_type,tags,relationships,metadata".to_string()];
    for issue in sorted_by_id(issues) {
        let description = match &issue.description {
            Some(d) => d.clone(),
            None => String::new(),
        };
        let row = [
            csv_encode_field(&issue.id.to_string()),
            csv_encode_field(&issue.title),
            csv_encode_field(&description),
            csv_encode_field(&format!("{}", issue.status)),
            csv_encode_field(&format!("{}", issue.issue_type)),
            csv_encode_field(&serde_json::to_string(&issue.tags).unwrap()),
            csv_encode_field(&serde_json::to_string(&issue.relationships).unwrap()),
            csv_encode_field(&serde_json::to_string(&issue.metadata).unwrap()),
        ];
        lines.push(row.join(","));
    }
    let mut content = lines.join("\n");
    content.push('\n');
    content
}

/// The checkbox glyph for an issue in the markdown export: `[x]` for a
/// done issue, `[ ]` otherwise.
fn checkbox(status: &Status) -> &'static str {
    match status {
        Status::Done => "x",
        _ => " ",
    }
}

/// Serialize `issues` as a readable Markdown document (r[export-markdown]):
/// one section per epic (children listed under it), then an Orphans
/// section for every non-epic issue that is not a child of any epic in the
/// export. Epics, children, and orphans each sort by title
/// (r[export-order]). Each row is `- [ ] <id> [<type>] [<status>]
/// <title>` with `[x]` for done issues. Best-effort and for humans:
/// relationships other than the epic grouping, tags, metadata, and
/// descriptions are not rendered -- use `export json` or `export csv` for
/// machine-readable data. Pure: no storage access.
pub fn export_markdown(issues: &[Issue]) -> String {
    let epics = sorted_by_title(
        &issues
            .iter()
            .filter(|i| i.issue_type == IssueType::Epic)
            .cloned()
            .collect::<Vec<Issue>>(),
    );

    let mut lines: Vec<String> = vec!["# Pebbles Export".to_string(), "".to_string()];
    let mut children_rendered = HashSet::new();
    for epic in &epics {
        lines.push(format!("## Epic: {}", epic.title));
        lines.push("".to_string());
        let children = sorted_by_title(
            &issues
                .iter()
                .filter(|i| {
                    i.relationships.iter().any(|rel| {
                        rel.relation_type == RelationType::PartOf && rel.target_id == epic.id
                    })
                })
                .cloned()
                .collect::<Vec<Issue>>(),
        );
        for child in &children {
            children_rendered.insert(child.id);
            lines.push(format!(
                "- [{}] {} [{}] [{}] {}",
                checkbox(&child.status),
                child.id,
                child.issue_type.short_code(),
                child.status,
                child.title
            ));
        }
        lines.push("".to_string());
    }

    lines.push("## Orphans".to_string());
    lines.push("".to_string());
    let orphans = sorted_by_title(
        &issues
            .iter()
            .filter(|i| i.issue_type != IssueType::Epic && !children_rendered.contains(&i.id))
            .cloned()
            .collect::<Vec<Issue>>(),
    );
    for orphan in &orphans {
        lines.push(format!(
            "- [{}] {} [{}] [{}] {}",
            checkbox(&orphan.status),
            orphan.id,
            orphan.issue_type.short_code(),
            orphan.status,
            orphan.title
        ));
    }
    lines.push("".to_string());

    lines.join("\n")
}

/// Parse and validate a pebbles export JSON bundle (r[export-bundle]): the
/// envelope must be an object with `"format": "pebbles-export"`,
/// `"version": 1`, and an `"issues"` array. Every issue element must pass
/// the same schema checks as a loaded issue file (a `serde`
/// deserialization into [`Issue`]); an invalid element fails the whole
/// import naming its 1-based position. An element with keys outside the
/// schema is imported with a warning, mirroring the store's load path; the
/// keys are dropped on write. Pure: no storage access.
pub fn parse_export_bundle(content: &str) -> Result<ImportPlan> {
    let root: Value = match serde_json::from_str::<Value>(content) {
        Ok(value) => value,
        Err(_) => bail!(
            "Not a pebbles export bundle: the file is not valid JSON (expected a JSON object with \
             \"format\", \"version\", and \"issues\")"
        ),
    };
    if !root.is_object() {
        bail!(
            "Not a pebbles export bundle: expected a JSON object with \"format\", \"version\", \
             and \"issues\""
        );
    }
    let format_value = root
        .get("format")
        .ok_or_else(|| anyhow::anyhow!("Not a pebbles export bundle: missing \"format\" field"))?;
    if format_value.as_str() != Some(EXPORT_FORMAT) {
        bail!("Not a pebbles export bundle: \"format\" must be \"pebbles-export\"");
    }
    let version_value = root
        .get("version")
        .ok_or_else(|| anyhow::anyhow!("Not a pebbles export bundle: missing \"version\" field"))?;
    match version_value.as_u64() {
        Some(v) if v != EXPORT_VERSION => bail!(
            "Unsupported export bundle version {}: this pebbles imports version {}",
            v,
            EXPORT_VERSION
        ),
        None => bail!("Not a pebbles export bundle: \"version\" must be a number"),
        Some(_) => {}
    }
    let Some(issues_value) = root.get("issues") else {
        bail!("Not a pebbles export bundle: missing \"issues\" field");
    };
    let Some(items) = issues_value.as_array() else {
        bail!("Not a pebbles export bundle: \"issues\" must be an array");
    };

    let mut issues: Vec<Issue> = Vec::new();
    let mut warnings: Vec<LoadWarning> = Vec::new();
    let mut n = 0usize;
    for item in items {
        n += 1;
        let issue_json = serde_json::to_string(item)?;
        let issue = match serde_json::from_str::<Issue>(&issue_json) {
            Ok(issue) => issue,
            Err(e) => bail!("Issue #{} in the export bundle is invalid: {}", n, e),
        };
        let unknown = unknown_top_level_keys(&issue_json);
        if !unknown.is_empty() {
            warnings.push(LoadWarning {
                file_name: String::new(),
                message: format!(
                    "Importing issue {}: unknown key(s) not in the schema: {}; the key(s) will be \
                     dropped on import",
                    issue.id,
                    unknown.join(", ")
                ),
            });
        }
        issues.push(issue);
    }
    Ok(ImportPlan { issues, warnings })
}

/// Reject an import before any write (r[import-semantics]): the bundle must
/// not contain duplicate issue ids, and no bundle issue may collide with an
/// id already in the store (import is additive and never overwrites).
/// Pure: no storage access.
///
/// ```rust
/// # use pebbles::export::validate_import;
/// # use pebbles::models::{Issue, IssueType};
/// # use uuid::Uuid;
/// let mut a = Issue::new("A".to_string(), None, IssueType::Bug);
/// a.id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
/// let mut b = Issue::new("B".to_string(), None, IssueType::Bug);
/// b.id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
/// // A duplicate inside the bundle is rejected.
/// assert!(validate_import(&[a.clone(), a.clone()], &[]).is_err());
/// // A clash with an existing store id is rejected.
/// assert!(validate_import(std::slice::from_ref(&a), &[a.id]).is_err());
/// // Distinct ids import cleanly.
/// assert!(validate_import(&[a.clone(), b.clone()], &[]).is_ok());
/// ```
pub fn validate_import(issues: &[Issue], existing_ids: &[Uuid]) -> Result<()> {
    let mut seen = HashSet::new();
    for issue in issues {
        if !seen.insert(issue.id) {
            bail!("Export bundle contains duplicate issue id: {}", issue.id);
        }
        if existing_ids.contains(&issue.id) {
            bail!(
                "Issue {} already exists in this store; import is additive and never \
                 overwrites existing issues",
                issue.id
            );
        }
    }
    Ok(())
}

/// Drop every relationship whose target is not in `known_ids` -- the union
/// of the bundle's ids and the store's existing ids -- returning one
/// LoadWarning per dropped relationship (r[import-semantics]). Keeps the
/// store's no-broken-links invariant (r[impl no-broken-links]): an imported
/// issue never points at a target that does not exist after the import.
/// Pure: no storage access.
///
/// ```rust
/// # use pebbles::export::drop_missing_relationships;
/// # use pebbles::models::{Issue, IssueType, RelationType, Relationship};
/// # use std::collections::HashSet;
/// # use uuid::Uuid;
/// let mut issue = Issue::new("A".to_string(), None, IssueType::Bug);
/// let present = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
/// let missing = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
/// issue.relationships.push(Relationship { relation_type: RelationType::RequiredFor, target_id: present });
/// issue.relationships.push(Relationship { relation_type: RelationType::RequiredFor, target_id: missing });
/// let mut known = HashSet::new();
/// known.insert(present);
/// let issues = vec![issue];
/// let (cleaned, warnings) = drop_missing_relationships(&issues, &known);
/// assert_eq!(warnings.len(), 1);
/// assert_eq!(cleaned[0].relationships.len(), 1);
/// assert_eq!(cleaned[0].relationships[0].target_id, present);
/// ```
pub fn drop_missing_relationships(
    issues: &[Issue],
    known_ids: &HashSet<Uuid>,
) -> (Vec<Issue>, Vec<LoadWarning>) {
    let mut out: Vec<Issue> = Vec::new();
    let mut warnings: Vec<LoadWarning> = Vec::new();
    for issue in issues {
        let mut clean = issue.clone();
        let dropped: Vec<Relationship> = clean
            .relationships
            .iter()
            .filter(|rel| !known_ids.contains(&rel.target_id))
            .cloned()
            .collect();
        if dropped.is_empty() {
            out.push(clean);
            continue;
        }
        clean
            .relationships
            .retain(|rel| known_ids.contains(&rel.target_id));
        out.push(clean);
        for rel in &dropped {
            warnings.push(LoadWarning {
                file_name: String::new(),
                message: format!(
                    "Import dropped relationship {} -> {} from '{}': target not found in the \
                     bundle or the store",
                    rel.relation_type, rel.target_id, issue.title
                ),
            });
        }
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Relationship;

    fn issue_with_id(id: &str) -> Issue {
        let mut issue = Issue::new("Issue".to_string(), None, IssueType::Bug);
        issue.id = Uuid::parse_str(id).unwrap();
        issue
    }

    fn issue_json(issue: &Issue) -> String {
        serde_json::to_string(issue).unwrap()
    }

    #[test]
    fn export_json_envelope_round_trips_every_field() {
        let mut a = issue_with_id("33333333-3333-3333-3333-333333333333");
        a.title = "Login button".to_string();
        a.description = Some("With a comma, a \"quote\", and a newline".to_string());
        a.status = Status::Todo;
        a.issue_type = IssueType::Usecase;
        a.tags = vec!["mobile".to_string(), "frontend".to_string()];
        a.metadata
            .insert("priority".to_string(), "high".to_string());
        let target = issue_with_id("11111111-1111-1111-1111-111111111111");
        a.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: target.id,
        });
        let b = issue_with_id("22222222-2222-2222-2222-222222222222");

        let json = export_json(&[a.clone(), b.clone()]);
        let plan = parse_export_bundle(&json).unwrap();
        assert!(plan.warnings.is_empty());
        assert_eq!(plan.issues.len(), 2);
        assert_eq!(issue_json(&plan.issues[0]), issue_json(&b));
        assert_eq!(issue_json(&plan.issues[1]), issue_json(&a));
        assert!(json.contains("\"format\": \"pebbles-export\""));
        assert!(json.contains("\"version\": 1"));
    }

    #[test]
    fn export_json_sorts_by_id() {
        let a = issue_with_id("33333333-3333-3333-3333-333333333333");
        let b = issue_with_id("11111111-1111-1111-1111-111111111111");
        let c = issue_with_id("22222222-2222-2222-2222-222222222222");
        let json = export_json(&[a, b, c]);
        let plan = parse_export_bundle(&json).unwrap();
        let ids: Vec<String> = plan
            .issues
            .iter()
            .map(|i| i.id.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "11111111-1111-1111-1111-111111111111".to_string(),
                "22222222-2222-2222-2222-222222222222".to_string(),
                "33333333-3333-3333-3333-333333333333".to_string(),
            ]
        );
    }

    #[test]
    fn csv_encode_field_quotes_and_doubles_quotes() {
        assert_eq!(csv_encode_field("plain"), "\"plain\"");
        assert_eq!(csv_encode_field("a,b \"c\""), "\"a,b \"\"c\"\"\"");
        assert_eq!(csv_encode_field("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn export_csv_has_header_and_rfc4180_rows() {
        let mut a = issue_with_id("11111111-1111-1111-1111-111111111111");
        a.title = "Login, with comma".to_string();
        a.description = Some("has a \"quote\"".to_string());
        a.status = Status::InProgress;
        a.issue_type = IssueType::UxProblem;
        a.tags = vec!["x".to_string(), "y".to_string()];
        let csv = export_csv(&[a]);
        let lines: Vec<&str> = csv.trim().split('\n').collect::<Vec<_>>();
        assert_eq!(
            lines[0],
            "id,title,description,status,issue_type,tags,relationships,metadata"
        );
        // The JSON-encoded tags are themselves inside a quoted, escaped CSV
        // field: inner quotes are doubled per RFC-4180.
        assert_eq!(
            lines[1],
            "\"11111111-1111-1111-1111-111111111111\",\"Login, with comma\",\"has a \"\"quote\"\"\",\"in_progress\",\"ux_problem\",\"[\"\"x\"\",\"\"y\"\"]\",\"[]\",\"{}\""
        );
    }

    #[test]
    fn export_markdown_groups_epics_and_orphans() {
        let mut epic = issue_with_id("44444444-4444-4444-4444-444444444444");
        epic.issue_type = IssueType::Epic;
        epic.title = "User Authentication Epic".to_string();
        let mut child = issue_with_id("11111111-1111-1111-1111-111111111111");
        child.title = "Login button".to_string();
        child.status = Status::Done;
        child.relationships.push(Relationship {
            relation_type: RelationType::PartOf,
            target_id: epic.id,
        });
        let orphan = issue_with_id("22222222-2222-2222-2222-222222222222");
        let md = export_markdown(&[epic, child, orphan]);
        assert!(md.contains("## Epic: User Authentication Epic"));
        assert!(md.contains("## Orphans"));
        assert!(md.contains("- [x] 11111111-1111-1111-1111-111111111111 [bug] [done] Login button"));
        assert!(md.contains("- [ ] 22222222-2222-2222-2222-222222222222 [bug] [proposed] Issue"));
        // The child appears under the epic only, never in Orphans.
        let parts: Vec<&str> = md.split("## Orphans").collect::<Vec<_>>();
        assert!(parts.len() == 2);
        assert!(!parts[1]
            .to_string()
            .contains("11111111-1111-1111-1111-111111111111"));
    }

    #[test]
    fn parse_export_bundle_rejects_non_bundles() {
        assert!(parse_export_bundle("not json").is_err());
        assert!(parse_export_bundle("42").is_err());
        assert!(
            parse_export_bundle(r#"{ "format": "other", "version": 1, "issues": [] }"#).is_err()
        );
        assert!(parse_export_bundle(
            r#"{ "format": "pebbles-export", "version": 2, "issues": [] }"#
        )
        .is_err());
        assert!(parse_export_bundle(r#"{ "format": "pebbles-export", "version": 1 }"#).is_err());
        assert!(parse_export_bundle(
            r#"{ "format": "pebbles-export", "version": 1, "issues": "nope" }"#
        )
        .is_err());
    }

    #[test]
    fn parse_export_bundle_rejects_invalid_issue_naming_position() {
        let err = parse_export_bundle(
            r#"{ "format": "pebbles-export", "version": 1, "issues": [ 17 ] }"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Issue #1"), "{:?}", err);
    }

    #[test]
    fn parse_export_bundle_warns_on_unknown_keys() {
        let json = r#"{
            "format": "pebbles-export",
            "version": 1,
            "issues": [ {
                "id": "11111111-1111-1111-1111-111111111111",
                "title": "T",
                "description": null,
                "status": "proposed",
                "issue_type": "bug",
                "tags": [],
                "relationships": [],
                "metadata": {},
                "created_at": "2026-01-10T08:00:00Z",
                "updated_at": "2026-01-10T08:00:00Z",
                "assignee": "alice"
            } ]
        }"#;
        let plan = parse_export_bundle(json).unwrap();
        assert_eq!(plan.issues.len(), 1);
        assert_eq!(plan.warnings.len(), 1);
        assert!(plan.warnings[0].message.contains("assignee"));
    }

    #[test]
    fn validate_import_rejects_duplicates_and_clashes() {
        let a = issue_with_id("11111111-1111-1111-1111-111111111111");
        let b = issue_with_id("22222222-2222-2222-2222-222222222222");
        assert!(validate_import(&[a.clone(), a.clone()], &[]).is_err());
        assert!(validate_import(std::slice::from_ref(&a), &[a.id]).is_err());
        assert!(validate_import(&[a.clone(), b], &[]).is_ok());
    }

    #[test]
    fn drop_missing_relationships_drops_and_warns() {
        let mut issue = issue_with_id("11111111-1111-1111-1111-111111111111");
        let present = issue_with_id("22222222-2222-2222-2222-222222222222");
        let missing = issue_with_id("99999999-9999-9999-9999-999999999999");
        issue.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: present.id,
        });
        issue.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: missing.id,
        });
        let mut known = HashSet::new();
        known.insert(present.id);
        let (cleaned, warnings) = drop_missing_relationships(&[issue], &known);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .message
            .contains("99999999-9999-9999-9999-999999999999"));
        assert_eq!(cleaned[0].relationships.len(), 1);
        assert_eq!(cleaned[0].relationships[0].target_id, present.id);
    }
}
