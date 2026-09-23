use crate::models::{build_epic_counts, Issue, IssueType, RelationType, Status};
use crate::operations::{BlockerStatus, CreationGuidance, ProjectSummary};
use colored::*;
use std::collections::HashMap;
use uuid::Uuid;

/// Format an epic's child progress as ` (done/total)`.
///
/// `filtered` counts are computed over the view slice; `full` over the
/// complete issue set. When a filter is applied, both ratios are shown:
/// ` (done/total, done/total total)`. See SPEC r[epic-progress-counts].
fn format_epic_count(
    filtered: Option<(usize, usize)>,
    full: Option<(usize, usize)>,
    filter_applied: bool,
) -> String {
    let ratio = |counts: Option<(usize, usize)>| match counts {
        Some((total, done)) => format!("{}/{}", done, total),
        None => "0/0".to_string(),
    };

    if filter_applied {
        format!(" ({}, {} total)", ratio(filtered), ratio(full))
    } else {
        format!(" ({})", ratio(full.or(filtered)))
    }
}

/// Render the dependency/blocker indicator (SPEC r[blocker-indicators]).
///
/// Returns the trailing row text for one issue:
/// - " ✓" (check) when the issue has no incomplete `required_for`
///   dependency and is itself still open (not Done);
/// - " ⚠ (blocked by: <ids>)" when it depends on incomplete issues;
/// - " → (blocking: <ids>)" when other open issues depend on it.
/// An issue that is both blocked and blocking only shows the blocked
/// marker; done issues show no marker at all.
///
/// Pure: no I/O, so the indicator rendering is unit-testable.
fn format_blocker_indicator(
    status: &BlockerStatus,
    display_status: Status,
    id_prefixes: &HashMap<Uuid, String>,
) -> String {
    if display_status == Status::Done {
        return String::new();
    }
    let prefix = |id: &Uuid| {
        id_prefixes
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string()[..8].to_string())
    };
    if !status.blocked_by.is_empty() {
        let ids: Vec<String> = status.blocked_by.iter().map(prefix).collect();
        format!(" ⚠ (blocked by: {})", ids.join(", "))
            .yellow()
            .to_string()
    } else if !status.blocking.is_empty() {
        let ids: Vec<String> = status.blocking.iter().map(prefix).collect();
        format!(" → (blocking: {})", ids.join(", "))
            .cyan()
            .to_string()
    } else if status.ready {
        " ✓".green().to_string()
    } else {
        String::new()
    }
}

pub fn print_issue_list(
    issues: &[Issue],
    full_epic_counts: &HashMap<Uuid, (usize, usize)>,
    filter_applied: bool,
    id_prefixes: &HashMap<Uuid, String>,
    blocker_statuses: &HashMap<Uuid, BlockerStatus>,
) {
    if issues.is_empty() {
        println!("{}", "No issues found.".dimmed());
        return;
    }

    // Epic counts over the view slice; full-set counts come from the caller.
    let epic_counts = build_epic_counts(issues);

    for issue in issues {
        let status_str = match issue.status {
            Status::Proposed => "PROPOSED".white(),
            Status::Todo => "TODO".yellow(),
            Status::InProgress => "IN PROGRESS".blue(),
            Status::Done => "DONE".green(),
            Status::Blocked => "BLOCKED".red(),
        };

        let type_str = match issue.issue_type {
            IssueType::Bug => "bug".red(),
            IssueType::UxProblem => "ux".magenta(),
            IssueType::Usecase => "uc".cyan(),
            IssueType::Design => "des".blue(),
            IssueType::Validation => "val".green(),
            IssueType::Epic => "epic".yellow().bold(),
            IssueType::Milestone => "mile".yellow().bold(),
        };

        let id_short = id_prefixes
            .get(&issue.id)
            .cloned()
            .unwrap_or_else(|| issue.short_id());

        // Add epic count for epics
        let count_str = if issue.issue_type == IssueType::Epic {
            format_epic_count(
                epic_counts.get(&issue.id).copied(),
                full_epic_counts.get(&issue.id).copied(),
                filter_applied,
            )
            .dimmed()
            .to_string()
        } else {
            String::new()
        };

        let type_badge = format!("[{}]", type_str);
        let status_badge = format!("[{}]", status_str);
        // r[blocker-indicators] - per-row dependency/blocker marker.
        let indicator = blocker_statuses
            .get(&issue.id)
            .map(|s| format_blocker_indicator(s, issue.status, id_prefixes))
            .unwrap_or_default();
        println!(
            "{} {} {} {}{}{}",
            id_short.dimmed(),
            type_badge,
            status_badge,
            issue.title.bold(),
            indicator,
            count_str
        );
    }
}

pub fn print_issue_detail(
    issue: &Issue,
    related_titles: &HashMap<Uuid, String>,
    incoming_relationships: &[(Uuid, String, RelationType)],
) {
    let status_str = match issue.status {
        Status::Proposed => "PROPOSED".white(),
        Status::Todo => "TODO".yellow(),
        Status::InProgress => "IN PROGRESS".blue(),
        Status::Done => "DONE".green(),
        Status::Blocked => "BLOCKED".red(),
    };

    let type_str = format!("{:?}", issue.issue_type);

    println!("{}", "━".repeat(60).dimmed());
    println!("{}: {}", "ID".bold(), issue.id.to_string().dimmed());
    println!("{}: {}", "Title".bold(), issue.title);
    println!("{}: {}", "Type".bold(), type_str.cyan());
    println!("{}: {}", "Status".bold(), status_str);

    if let Some(desc) = &issue.description {
        println!("{}: {}", "Description".bold(), desc);
    }

    if !issue.tags.is_empty() {
        println!("{}: {}", "Tags".bold(), issue.tags.join(", "));
    }

    // Show outgoing relationships (this issue -> other)
    if !issue.relationships.is_empty() {
        println!("\n{}", "Relationships:".bold());
        for rel in &issue.relationships {
            let rel_str = match rel.relation_type {
                RelationType::PartOf => "part of",
                RelationType::RequiredFor => "required for",
                RelationType::ScheduledBefore => "scheduled before",
            };
            let id_display = rel.target_id.to_string()[..8].to_string().cyan();

            if let Some(title) = related_titles.get(&rel.target_id) {
                println!("  {} {} ({})", rel_str.dimmed(), id_display, title.bold());
            } else {
                println!("  {} {}", rel_str.dimmed(), id_display);
            }
        }
    }

    // Show incoming relationships (other -> this issue)
    if !incoming_relationships.is_empty() {
        println!("\n{}", "Referenced by:".bold());
        for (source_id, source_title, rel_type) in incoming_relationships {
            let rel_str = match rel_type {
                RelationType::PartOf => "has part",
                RelationType::RequiredFor => "requires",
                RelationType::ScheduledBefore => "scheduled after",
            };
            let id_display = source_id.to_string()[..8].to_string().cyan();
            println!(
                "  {} {} ({})",
                rel_str.dimmed(),
                id_display,
                source_title.bold()
            );
        }
    }

    if !issue.metadata.is_empty() {
        println!("\n{}", "Metadata:".bold());
        for (key, value) in &issue.metadata {
            println!("  {}: {}", key.dimmed(), value);
        }
    }

    println!(
        "\n{}: {}",
        "Created".bold(),
        issue.created_at.format("%Y-%m-%d %H:%M:%S")
    );
    println!(
        "{}: {}",
        "Updated".bold(),
        issue.updated_at.format("%Y-%m-%d %H:%M:%S")
    );
    println!("{}: {}", "File".bold(), issue.file_name().dimmed());
    println!("{}", "━".repeat(60).dimmed());
}

/// Print the optional guidance tips shown after a successful issue
/// creation (r[guidance-tips]). Hints, never errors: each tip is one line
/// prefixed `Tip:`, followed by the pickable epic candidates when the epic
/// tip applies. Renders nothing at all when no tip applies, so creation
/// output stays byte-identical for projects without epics.
pub fn print_creation_tips(
    guidance: &CreationGuidance,
    new_issue_short_id: &str,
    id_prefixes: &HashMap<Uuid, String>,
) {
    if guidance.suggest_epic {
        println!();
        println!(
            "Tip: Link to an epic with: pebbles link {} part_of <epic-id>",
            new_issue_short_id
        );
        for (id, title) in &guidance.epic_candidates {
            let short = id_prefixes
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string());
            println!("  {} {}", short.dimmed(), title.bold());
        }
    }
    if guidance.suggest_dependency {
        if !guidance.suggest_epic {
            println!();
        }
        println!(
            "Tip: Add dependencies with: pebbles link {} required_for <other-id>",
            new_issue_short_id
        );
    }
}

pub fn print_epic_view(epic: &Issue, child_issues: &[Issue], id_prefixes: &HashMap<Uuid, String>) {
    let status_str = match epic.status {
        Status::Proposed => "PROPOSED".white(),
        Status::Todo => "TODO".yellow(),
        Status::InProgress => "IN PROGRESS".blue(),
        Status::Done => "DONE".green(),
        Status::Blocked => "BLOCKED".red(),
    };

    println!("{}", "━".repeat(60).dimmed());
    println!("{}: {}", "Epic".bold(), epic.title.bold());
    println!(
        "{}: {}",
        "ID".bold(),
        id_prefixes
            .get(&epic.id)
            .cloned()
            .unwrap_or_else(|| epic.short_id())
            .cyan()
    );
    println!("{}: {}", "Status".bold(), status_str);

    if let Some(desc) = &epic.description {
        println!("\n{}", desc);
    }

    let done_count = child_issues
        .iter()
        .filter(|i| i.status == Status::Done)
        .count();
    let total_count = child_issues.len();
    println!("\n{}: {}/{}", "Progress".bold(), done_count, total_count);

    println!("{}", "━".repeat(60).dimmed());

    if child_issues.is_empty() {
        println!("\n{}", "No child issues found.".dimmed());
    } else {
        println!("\n{}:", "Child Issues".bold());
        for issue in child_issues {
            let status_str = match issue.status {
                Status::Proposed => "PROPOSED".white(),
                Status::Todo => "TODO".yellow(),
                Status::InProgress => "IN PROGRESS".blue(),
                Status::Done => "DONE".green(),
                Status::Blocked => "BLOCKED".red(),
            };

            let type_str = match issue.issue_type {
                IssueType::Bug => "bug".red(),
                IssueType::UxProblem => "ux".magenta(),
                IssueType::Usecase => "uc".cyan(),
                IssueType::Design => "des".blue(),
                IssueType::Validation => "val".green(),
                IssueType::Epic => "epic".yellow().bold(),
                IssueType::Milestone => "mile".yellow().bold(),
            };

            let type_badge = format!("[{}]", type_str);
            let status_badge = format!("[{}]", status_str);
            println!(
                "  {} {} {} {}",
                id_prefixes
                    .get(&issue.id)
                    .cloned()
                    .unwrap_or_else(|| issue.short_id())
                    .dimmed(),
                type_badge,
                status_badge,
                issue.title
            );
        }
    }

    println!();
}

pub fn print_issue_tree(
    issues: &[Issue],
    full_epic_counts: &HashMap<Uuid, (usize, usize)>,
    filter_applied: bool,
    id_prefixes: &HashMap<Uuid, String>,
) {
    if issues.is_empty() {
        println!("{}", "No issues found.".dimmed());
        return;
    }

    // Separate epics and non-epics
    let epics: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.issue_type == IssueType::Epic)
        .collect();
    let non_epics: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.issue_type != IssueType::Epic)
        .collect();

    // Epic counts over the view slice; full-set counts come from the caller.
    let slice: Vec<Issue> = non_epics.iter().map(|i| (*i).clone()).collect();
    let epic_counts = build_epic_counts(&slice);

    // Display epics with their children
    for epic in &epics {
        let status_str = match epic.status {
            Status::Proposed => "PROPOSED".white(),
            Status::Todo => "TODO".yellow(),
            Status::InProgress => "IN PROGRESS".blue(),
            Status::Done => "DONE".green(),
            Status::Blocked => "BLOCKED".red(),
        };

        let count_str = format_epic_count(
            epic_counts.get(&epic.id).copied(),
            full_epic_counts.get(&epic.id).copied(),
            filter_applied,
        )
        .dimmed()
        .to_string();

        let status_badge = format!("[{}]", status_str);
        println!(
            "{} {} {} {}{}",
            id_prefixes
                .get(&epic.id)
                .cloned()
                .unwrap_or_else(|| epic.short_id())
                .dimmed(),
            "[epic]".yellow().bold(),
            status_badge,
            epic.title.bold(),
            count_str
        );

        // Find and display children
        let mut children: Vec<&Issue> = non_epics
            .iter()
            .filter(|issue| {
                issue.relationships.iter().any(|rel| {
                    rel.relation_type == crate::models::RelationType::PartOf
                        && rel.target_id == epic.id
                })
            })
            .copied()
            .collect();

        children.sort_by_key(|issue| std::cmp::Reverse(issue.updated_at));

        for (idx, child) in children.iter().enumerate() {
            let is_last = idx == children.len() - 1;
            let prefix = if is_last { "└─" } else { "├─" };

            let status_str = match child.status {
                Status::Proposed => "PROPOSED".white(),
                Status::Todo => "TODO".yellow(),
                Status::InProgress => "IN PROGRESS".blue(),
                Status::Done => "DONE".green(),
                Status::Blocked => "BLOCKED".red(),
            };

            let type_str = match child.issue_type {
                IssueType::Bug => "bug".red(),
                IssueType::UxProblem => "ux".magenta(),
                IssueType::Usecase => "uc".cyan(),
                IssueType::Design => "des".blue(),
                IssueType::Validation => "val".green(),
                IssueType::Epic => "epic".yellow().bold(),
                IssueType::Milestone => "mile".yellow().bold(),
            };

            let type_badge = format!("[{}]", type_str);
            let status_badge = format!("[{}]", status_str);
            println!(
                "  {} {} {} {} {}",
                prefix.dimmed(),
                id_prefixes
                    .get(&child.id)
                    .cloned()
                    .unwrap_or_else(|| child.short_id())
                    .dimmed(),
                type_badge,
                status_badge,
                child.title
            );
        }

        if !children.is_empty() {
            println!();
        }
    }

    // Display orphaned issues (not part of any epic)
    let orphans: Vec<&Issue> = non_epics
        .iter()
        .filter(|issue| {
            !issue
                .relationships
                .iter()
                .any(|rel| rel.relation_type == crate::models::RelationType::PartOf)
        })
        .copied()
        .collect();

    if !orphans.is_empty() {
        if !epics.is_empty() {
            println!("{}", "Orphaned Issues:".bold());
        }

        for issue in orphans {
            let status_str = match issue.status {
                Status::Proposed => "PROPOSED".white(),
                Status::Todo => "TODO".yellow(),
                Status::InProgress => "IN PROGRESS".blue(),
                Status::Done => "DONE".green(),
                Status::Blocked => "BLOCKED".red(),
            };

            let type_str = match issue.issue_type {
                IssueType::Bug => "bug".red(),
                IssueType::UxProblem => "ux".magenta(),
                IssueType::Usecase => "uc".cyan(),
                IssueType::Design => "des".blue(),
                IssueType::Validation => "val".green(),
                IssueType::Epic => "epic".yellow().bold(),
                IssueType::Milestone => "mile".yellow().bold(),
            };

            let type_badge = format!("[{}]", type_str);
            let status_badge = format!("[{}]", status_str);
            println!(
                "{} {} {} {}",
                id_prefixes
                    .get(&issue.id)
                    .cloned()
                    .unwrap_or_else(|| issue.short_id())
                    .dimmed(),
                type_badge,
                status_badge,
                issue.title.bold()
            );
        }
    }
}

pub fn print_summary(summary: &ProjectSummary) {
    println!("{}", "═".repeat(60).bold());
    println!("{}", "Project Overview".bold());
    println!("{}", "═".repeat(60).bold());
    println!();

    println!("{}: {}", "Total issues".bold(), summary.total_issues);
    println!();

    // By Status
    println!("{}:", "By Status".bold());
    let statuses = [
        Status::Proposed,
        Status::Todo,
        Status::InProgress,
        Status::Blocked,
        Status::Done,
    ];
    for status in &statuses {
        let count = summary.by_status.get(status).unwrap_or(&0);
        let percentage = if summary.total_issues > 0 {
            (*count as f32 / summary.total_issues as f32 * 100.0) as usize
        } else {
            0
        };

        let status_str = match status {
            Status::Proposed => "PROPOSED".white(),
            Status::Todo => "TODO".yellow(),
            Status::InProgress => "IN PROGRESS".blue(),
            Status::Done => "DONE".green(),
            Status::Blocked => "BLOCKED".red(),
        };

        println!("  {}: {} ({}%)", status_str, count, percentage);
    }
    println!();

    // By Type
    println!("{}:", "By Type".bold());
    let types = [
        IssueType::Bug,
        IssueType::UxProblem,
        IssueType::Usecase,
        IssueType::Design,
        IssueType::Validation,
        IssueType::Epic,
        IssueType::Milestone,
    ];
    for issue_type in &types {
        if let Some(count) = summary.by_type.get(issue_type) {
            if *count > 0 {
                let type_str = match issue_type {
                    IssueType::Bug => "Bugs".red(),
                    IssueType::UxProblem => "UX Problems".magenta(),
                    IssueType::Usecase => "Usecases".cyan(),
                    IssueType::Design => "Design".blue(),
                    IssueType::Validation => "Validation".green(),
                    IssueType::Epic => "Epics".yellow().bold(),
                    IssueType::Milestone => "Milestones".yellow().bold(),
                };
                println!("  {}: {}", type_str, count);
            }
        }
    }
    println!();

    // Epics
    if !summary.epics.is_empty() {
        println!("{}: {} total", "Epics".bold(), summary.epics.len());
        for (epic, total, done) in &summary.epics {
            let progress = if *total > 0 {
                (*done as f32 / *total as f32 * 100.0) as usize
            } else {
                100
            };

            let progress_str = if *done == *total && *total > 0 {
                format!("✓ {}/{} ({}%)", done, total, progress).green()
            } else {
                format!("{}/{} ({}%)", done, total, progress).dimmed()
            };

            println!("  {} {}", epic.title.bold(), progress_str);
        }
        println!();
    }

    // Relationships
    if summary.total_relationships > 0 {
        println!("{}:", "Relationships".bold());
        println!("  {}: {}", "Total".dimmed(), summary.total_relationships);
        let rel_types = [
            RelationType::PartOf,
            RelationType::RequiredFor,
            RelationType::ScheduledBefore,
        ];
        for rel_type in &rel_types {
            if let Some(count) = summary.by_relationship_type.get(rel_type) {
                if *count > 0 {
                    let rel_str = match rel_type {
                        RelationType::PartOf => "part_of",
                        RelationType::RequiredFor => "required_for",
                        RelationType::ScheduledBefore => "scheduled_before",
                    };
                    println!("  {}: {}", rel_str.dimmed(), count);
                }
            }
        }
        println!();
    }

    // Actionable
    if summary.actionable_count > 0 {
        println!(
            "{}: {} (use 'pebbles next' to see)",
            "Ready to work on".bold().green(),
            summary.actionable_count
        );
    } else {
        println!(
            "{}",
            "No actionable issues (all are blocked or done)".dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{format_blocker_indicator, format_epic_count};
    use crate::models::Status;
    use crate::operations::BlockerStatus;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    fn prefixes() -> HashMap<Uuid, String> {
        let mut m = HashMap::new();
        m.insert(
            uuid("11111111-2222-3333-4444-555555555555"),
            "111".to_string(),
        );
        m.insert(
            uuid("22222222-2222-3333-4444-555555555555"),
            "222".to_string(),
        );
        m
    }

    fn ready_status() -> BlockerStatus {
        BlockerStatus {
            ready: true,
            blocked_by: vec![],
            blocking: vec![],
        }
    }

    #[test]
    fn blocker_indicator_ready_shows_check() {
        let s = format_blocker_indicator(&ready_status(), Status::Todo, &prefixes());
        assert!(s.contains("✓"), "got: {s}");
    }

    #[test]
    fn blocker_indicator_done_shows_nothing() {
        let s = format_blocker_indicator(&ready_status(), Status::Done, &prefixes());
        assert_eq!(s, "");
    }

    #[test]
    fn blocker_indicator_blocked_shows_warn_with_ids() {
        let mut status = ready_status();
        status.ready = false;
        status.blocked_by = vec![
            uuid("11111111-2222-3333-4444-555555555555"),
            uuid("22222222-2222-3333-4444-555555555555"),
        ];
        let s = format_blocker_indicator(&status, Status::Todo, &prefixes());
        assert!(s.contains("(blocked by: 111, 222)"), "got: {s}");
    }

    #[test]
    fn blocker_indicator_blocking_shows_arrow_with_ids() {
        let mut status = ready_status();
        status.blocking = vec![uuid("11111111-2222-3333-4444-555555555555")];
        let s = format_blocker_indicator(&status, Status::Todo, &prefixes());
        assert!(s.contains("(blocking: 111)"), "got: {s}");
    }

    #[test]
    fn blocker_indicator_blocked_takes_precedence_over_blocking() {
        let mut status = ready_status();
        status.ready = false;
        status.blocked_by = vec![uuid("11111111-2222-3333-4444-555555555555")];
        status.blocking = vec![uuid("22222222-2222-3333-4444-555555555555")];
        let s = format_blocker_indicator(&status, Status::Todo, &prefixes());
        assert!(s.contains("(blocked by: 111)"), "got: {s}");
        assert!(!s.contains("blocking"), "got: {s}");
    }

    #[test]
    fn test_epic_count_format_is_always_done_over_total() {
        assert_eq!(
            format_epic_count(Some((3, 1)), Some((3, 1)), false),
            " (1/3)"
        );
        // A childless epic uses the same done/total shape.
        assert_eq!(format_epic_count(None, None, false), " (0/0)");
    }

    #[test]
    fn test_epic_count_without_filter_is_single_ratio() {
        // Without a filter the slice equals the full set: one ratio.
        assert_eq!(
            format_epic_count(Some((3, 1)), Some((3, 1)), false),
            " (1/3)"
        );
        assert_eq!(format_epic_count(None, None, false), " (0/0)");
    }

    // r[verify epic-progress-counts]
    #[test]
    fn test_epic_count_with_filter_shows_both_ratios() {
        // Filtered ratio first, then the complete-set ratio.
        assert_eq!(
            format_epic_count(Some((1, 0)), Some((3, 1)), true),
            " (0/1, 1/3 total)"
        );
        // All children filtered out: filtered ratio is 0/0.
        assert_eq!(
            format_epic_count(None, Some((3, 1)), true),
            " (0/0, 1/3 total)"
        );
    }
}
