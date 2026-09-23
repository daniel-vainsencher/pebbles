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

/// Terminal color for a relationship-health indicator (SPEC
/// r[blocker-colors]). The palette is fixed: green = ready, yellow =
/// blocked, red = circular dependency, blue = blocking others
/// (informational), gray = not part of any epic. Rendering is delegated to
/// the `colored` crate, which suppresses colors when stdout is not a
/// terminal, when `NO_COLOR` is set, or when `CLICOLOR=0` is set - so the
/// plain-text markers below never carry escape codes in those cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorColor {
    /// Issue has no blockers, ready to work on.
    Green,
    /// Issue is blocked by incomplete dependencies.
    Yellow,
    /// Issue is part of a circular dependency.
    Red,
    /// Issue is blocking other issues (informational).
    Blue,
}

/// A dependency/blocker marker: its plain text (the marker from SPEC
/// r[blocker-indicators]) plus the color to paint it (SPEC r[blocker-colors]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerMarker {
    /// The plain-text marker, e.g. `" ✓"` or `" ⚠ (blocked by: 111)"`.
    pub text: String,
    /// The color the marker renders in.
    pub color: IndicatorColor,
}

/// Select the dependency/blocker marker for one issue (SPEC r[blocker-indicators]
/// for the text, r[blocker-colors] for the color):
/// - " ✓" (green) when the issue has no incomplete `required_for`
///   dependency and is itself still open (not Done);
/// - " ⚠ (blocked by: <ids>)" (yellow) when it depends on incomplete issues;
/// - " ⛔ (circular)" (red) when it sits on a dependency cycle (a circular
///   issue is flagged even when it is also blocked);
/// - " → (blocking: <ids>)" (blue) when other open issues depend on it.
/// An issue that is both blocked and blocking only shows the blocked
/// marker; done issues show no marker at all.
///
/// Pure: no I/O and no color emission, so the selection is unit-testable
/// and the renderer decides whether to actually paint.
fn blocker_marker(
    status: &BlockerStatus,
    display_status: Status,
    id_prefixes: &HashMap<Uuid, String>,
) -> Option<BlockerMarker> {
    if display_status == Status::Done {
        return None;
    }
    let prefix = |id: &Uuid| {
        id_prefixes
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string()[..8].to_string())
    };
    if status.circular {
        Some(BlockerMarker {
            text: " ⛔ (circular)".to_string(),
            color: IndicatorColor::Red,
        })
    } else if !status.blocked_by.is_empty() {
        let ids: Vec<String> = status.blocked_by.iter().map(prefix).collect();
        Some(BlockerMarker {
            text: format!(" ⚠ (blocked by: {})", ids.join(", ")),
            color: IndicatorColor::Yellow,
        })
    } else if !status.blocking.is_empty() {
        let ids: Vec<String> = status.blocking.iter().map(prefix).collect();
        Some(BlockerMarker {
            text: format!(" → (blocking: {})", ids.join(", ")),
            color: IndicatorColor::Blue,
        })
    } else if status.ready {
        Some(BlockerMarker {
            text: " ✓".to_string(),
            color: IndicatorColor::Green,
        })
    } else {
        None
    }
}

/// Paint a marker's text with its indicator color. The `colored` crate
/// decides at print time whether escape codes are emitted (tty, `NO_COLOR`,
/// `CLICOLOR`), so this is the single place the palette maps to styles.
fn paint_indicator(color: IndicatorColor, text: &str) -> colored::ColoredString {
    match color {
        IndicatorColor::Green => text.green(),
        IndicatorColor::Yellow => text.yellow(),
        IndicatorColor::Red => text.red(),
        IndicatorColor::Blue => text.blue(),
    }
}

/// The color for an epic's progress count by completion percentage (SPEC
/// r[blocker-colors]): green at >=50% done, yellow below 50% with at least
/// one child done, red when nothing is done (a childless epic counts as 0%).
///
/// Pure: no I/O, so the selection is unit-testable.
pub fn epic_count_color(done: usize, total: usize) -> IndicatorColor {
    if total > 0 && done * 2 >= total {
        IndicatorColor::Green
    } else if total > 0 && done > 0 {
        IndicatorColor::Yellow
    } else {
        IndicatorColor::Red
    }
}

/// True when a non-epic issue row should render gray: the issue is not part
/// of any epic (`part_of` relationship) and the view contains epics (SPEC
/// r[blocker-colors]). Epics themselves are never grayed.
///
/// Pure: no I/O, so the selection is unit-testable.
pub fn epic_membership_gray(has_part_of: bool, epics_in_view: bool) -> bool {
    !has_part_of && epics_in_view
}

/// Health color for one outgoing relationship row in `pebbles show` (SPEC
/// r[blocker-colors]).
///
/// The detail view does not receive the relationship graph, so only
/// locally verifiable problems are flagged red: a self-referencing edge
/// (source == target, the minimal cycle) and a dangling target (the target
/// issue does not exist in the store). Longer dependency cycles are not
/// visible here - they are flagged by the list markers, which do see the
/// full graph (r[blocker-indicators]).
///
/// Pure: no I/O, so the selection is unit-testable.
pub fn show_relationship_health(
    source_id: &Uuid,
    target_id: &Uuid,
    target_exists: bool,
) -> Option<IndicatorColor> {
    if target_id == source_id || !target_exists {
        Some(IndicatorColor::Red)
    } else {
        None
    }
}

/// Render the dependency/blocker indicator (SPEC r[blocker-indicators]).
///
/// Returns the trailing row text for one issue, painted per
/// r[blocker-colors]; never emits escape codes when the terminal does not
/// support color (handled by the `colored` crate). An issue that is both
/// blocked and blocking only shows the blocked marker; done issues show no
/// marker at all.
///
/// Pure: no I/O, so the indicator rendering is unit-testable.
fn format_blocker_indicator(
    status: &BlockerStatus,
    display_status: Status,
    id_prefixes: &HashMap<Uuid, String>,
) -> String {
    blocker_marker(status, display_status, id_prefixes)
        .map(|m| paint_indicator(m.color, &m.text).to_string())
        .unwrap_or_default()
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
    // r[blocker-colors] - rows are grayed when the view has epics and the
    // issue belongs to none of them.
    let epics_in_view = issues.iter().any(|i| i.issue_type == IssueType::Epic);

    for issue in issues {
        // Plain names are kept next to the colored ones: gray rows need the
        // uncolored text because stacking SGR codes does not override the
        // badge's own color (r[blocker-colors]).
        let status_name = match issue.status {
            Status::Proposed => "PROPOSED",
            Status::Todo => "TODO",
            Status::InProgress => "IN PROGRESS",
            Status::Done => "DONE",
            Status::Blocked => "BLOCKED",
        };
        let type_name = match issue.issue_type {
            IssueType::Bug => "bug",
            IssueType::UxProblem => "ux",
            IssueType::Usecase => "uc",
            IssueType::Design => "des",
            IssueType::Validation => "val",
            IssueType::Epic => "epic",
            IssueType::Milestone => "mile",
        };
        let status_str = match issue.status {
            Status::Proposed => status_name.white(),
            Status::Todo => status_name.yellow(),
            Status::InProgress => status_name.blue(),
            Status::Done => status_name.green(),
            Status::Blocked => status_name.red(),
        };

        let type_str = match issue.issue_type {
            IssueType::Bug => type_name.red(),
            IssueType::UxProblem => type_name.magenta(),
            IssueType::Usecase => type_name.cyan(),
            IssueType::Design => type_name.blue(),
            IssueType::Validation => type_name.green(),
            IssueType::Epic => type_name.cyan().bold(),
            IssueType::Milestone => type_name.yellow().bold(),
        };

        let id_short = id_prefixes
            .get(&issue.id)
            .cloned()
            .unwrap_or_else(|| issue.short_id());

        // Add epic count for epics, colored by completion (r[blocker-colors]).
        let count_str = if issue.issue_type == IssueType::Epic {
            let count_str = format_epic_count(
                epic_counts.get(&issue.id).copied(),
                full_epic_counts.get(&issue.id).copied(),
                filter_applied,
            );
            let (total, done) = full_epic_counts
                .get(&issue.id)
                .or_else(|| epic_counts.get(&issue.id))
                .copied()
                .unwrap_or((0, 0));
            paint_indicator(epic_count_color(done, total), &count_str).to_string()
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
        // r[blocker-colors] - issues outside any epic render gray when the
        // view contains epics; the health marker stays colored.
        let grayed = issue.issue_type != IssueType::Epic
            && epic_membership_gray(
                issue
                    .relationships
                    .iter()
                    .any(|rel| rel.relation_type == RelationType::PartOf),
                epics_in_view,
            );
        if grayed {
            println!(
                "{} {} {} {}{}{}",
                id_short.bright_black(),
                format!("[{}]", type_name).bright_black(),
                format!("[{}]", status_name).bright_black(),
                issue.title.bright_black(),
                indicator,
                count_str
            );
        } else {
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
            // r[blocker-colors] - a self-referencing or dangling target is
            // an invalid relationship and renders red; everything else
            // keeps the cyan id.
            let raw_id = &rel.target_id.to_string()[..8];
            let id_display = match show_relationship_health(
                &issue.id,
                &rel.target_id,
                related_titles.contains_key(&rel.target_id),
            ) {
                Some(IndicatorColor::Red) => raw_id.red().to_string(),
                _ => raw_id.cyan().to_string(),
            };

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
        );
        // r[blocker-colors] - progress count colored by completion.
        let (total, done) = full_epic_counts
            .get(&epic.id)
            .or_else(|| epic_counts.get(&epic.id))
            .copied()
            .unwrap_or((0, 0));
        let count_str = paint_indicator(epic_count_color(done, total), &count_str).to_string();

        let status_badge = format!("[{}]", status_str);
        println!(
            "{} {} {} {}{}",
            id_prefixes
                .get(&epic.id)
                .cloned()
                .unwrap_or_else(|| epic.short_id())
                .dimmed(),
            "[epic]".cyan().bold(),
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

        // r[blocker-colors] - rows outside any epic render gray when the
        // view contains epics; with no epics the rows keep normal colors.
        let grayed = !epics.is_empty();
        for issue in orphans {
            let status_name = match issue.status {
                Status::Proposed => "PROPOSED",
                Status::Todo => "TODO",
                Status::InProgress => "IN PROGRESS",
                Status::Done => "DONE",
                Status::Blocked => "BLOCKED",
            };
            let type_name = match issue.issue_type {
                IssueType::Bug => "bug",
                IssueType::UxProblem => "ux",
                IssueType::Usecase => "uc",
                IssueType::Design => "des",
                IssueType::Validation => "val",
                IssueType::Epic => "epic",
                IssueType::Milestone => "mile",
            };
            let status_str = match issue.status {
                Status::Proposed => status_name.white(),
                Status::Todo => status_name.yellow(),
                Status::InProgress => status_name.blue(),
                Status::Done => status_name.green(),
                Status::Blocked => status_name.red(),
            };

            let type_str = match issue.issue_type {
                IssueType::Bug => type_name.red(),
                IssueType::UxProblem => type_name.magenta(),
                IssueType::Usecase => type_name.cyan(),
                IssueType::Design => type_name.blue(),
                IssueType::Validation => type_name.green(),
                IssueType::Epic => "epic".cyan().bold(),
                IssueType::Milestone => "mile".yellow().bold(),
            };

            let type_badge = format!("[{}]", type_str);
            let status_badge = format!("[{}]", status_str);
            if grayed {
                println!(
                    "{} {} {} {}",
                    id_prefixes
                        .get(&issue.id)
                        .cloned()
                        .unwrap_or_else(|| issue.short_id())
                        .bright_black(),
                    format!("[{}]", type_name).bright_black(),
                    format!("[{}]", status_name).bright_black(),
                    issue.title.bright_black()
                );
            } else {
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
    use super::{
        blocker_marker, epic_count_color, epic_membership_gray, format_blocker_indicator,
        format_epic_count, show_relationship_health, IndicatorColor,
    };
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
            circular: false,
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

    // r[verify blocker-colors] - the color-selection helpers are pure and
    // unit-tested; rendering applies them via the `colored` crate.
    #[test]
    fn blocker_marker_ready_is_green() {
        let m = blocker_marker(&ready_status(), Status::Todo, &prefixes()).unwrap();
        assert_eq!(m.color, IndicatorColor::Green);
        assert_eq!(m.text, " \u{2713}");
    }

    #[test]
    fn blocker_marker_blocked_is_yellow() {
        let mut status = ready_status();
        status.ready = false;
        status.blocked_by = vec![uuid("11111111-2222-3333-4444-555555555555")];
        let m = blocker_marker(&status, Status::Todo, &prefixes()).unwrap();
        assert_eq!(m.color, IndicatorColor::Yellow);
    }

    #[test]
    fn blocker_marker_blocking_is_blue() {
        let mut status = ready_status();
        status.blocking = vec![uuid("11111111-2222-3333-4444-555555555555")];
        let m = blocker_marker(&status, Status::Todo, &prefixes()).unwrap();
        assert_eq!(m.color, IndicatorColor::Blue);
    }

    #[test]
    fn blocker_marker_circular_is_red() {
        let mut status = ready_status();
        status.ready = false;
        status.blocked_by = vec![uuid("11111111-2222-3333-4444-555555555555")];
        status.circular = true;
        let m = blocker_marker(&status, Status::Todo, &prefixes()).unwrap();
        assert_eq!(m.color, IndicatorColor::Red);
        assert!(m.text.contains("circular"), "got: {}", m.text);
    }

    #[test]
    fn blocker_marker_circular_takes_precedence_over_blocked() {
        let mut status = ready_status();
        status.ready = false;
        status.blocked_by = vec![uuid("11111111-2222-3333-4444-555555555555")];
        status.blocking = vec![uuid("22222222-2222-3333-4444-555555555555")];
        status.circular = true;
        let m = blocker_marker(&status, Status::Todo, &prefixes()).unwrap();
        assert_eq!(m.color, IndicatorColor::Red);
    }

    #[test]
    fn blocker_marker_done_is_none() {
        assert!(blocker_marker(&ready_status(), Status::Done, &prefixes()).is_none());
    }

    #[test]
    fn epic_count_color_green_at_half_complete_or_more() {
        assert_eq!(epic_count_color(5, 10), IndicatorColor::Green);
        assert_eq!(epic_count_color(10, 10), IndicatorColor::Green);
    }

    #[test]
    fn epic_count_color_yellow_below_half() {
        assert_eq!(epic_count_color(4, 10), IndicatorColor::Yellow);
        assert_eq!(epic_count_color(1, 10), IndicatorColor::Yellow);
    }

    #[test]
    fn epic_count_color_red_when_nothing_done() {
        assert_eq!(epic_count_color(0, 10), IndicatorColor::Red);
        // A childless epic has 0% completion and is red like any 0/0.
        assert_eq!(epic_count_color(0, 0), IndicatorColor::Red);
    }

    #[test]
    fn epic_membership_gray_only_orphans_when_epics_exist() {
        assert!(epic_membership_gray(false, true));
        assert!(!epic_membership_gray(false, false));
        assert!(!epic_membership_gray(true, true));
        assert!(!epic_membership_gray(true, false));
    }

    #[test]
    fn show_relationship_self_reference_is_red() {
        let id = uuid("11111111-2222-3333-4444-555555555555");
        assert_eq!(
            show_relationship_health(&id, &id, true),
            Some(IndicatorColor::Red)
        );
    }

    #[test]
    fn show_relationship_dangling_target_is_red() {
        let src = uuid("11111111-2222-3333-4444-555555555555");
        let target = uuid("22222222-2222-3333-4444-555555555555");
        assert_eq!(
            show_relationship_health(&src, &target, false),
            Some(IndicatorColor::Red)
        );
    }

    #[test]
    fn show_relationship_healthy_target_has_no_color() {
        let src = uuid("11111111-2222-3333-4444-555555555555");
        let target = uuid("22222222-2222-3333-4444-555555555555");
        assert_eq!(show_relationship_health(&src, &target, true), None);
    }
}
