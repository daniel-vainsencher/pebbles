use crate::models::{Issue, IssueType, RelationType, Status};
use crate::operations::ProjectSummary;
use colored::*;
use std::collections::HashMap;
use uuid::Uuid;

pub fn print_issue_list(issues: &[Issue]) {
    if issues.is_empty() {
        println!("{}", "No issues found.".dimmed());
        return;
    }

    // Build epic counts: map epic_id -> (total, done)
    let mut epic_counts: HashMap<Uuid, (usize, usize)> = HashMap::new();
    for issue in issues {
        for rel in &issue.relationships {
            if rel.relation_type == crate::models::RelationType::PartOf {
                let entry = epic_counts.entry(rel.target_id).or_insert((0, 0));
                entry.0 += 1;
                if issue.status == Status::Done {
                    entry.1 += 1;
                }
            }
        }
    }

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

        let id_short = issue.short_id();

        // Add epic count for epics
        let count_str = if issue.issue_type == IssueType::Epic {
            if let Some((total, done)) = epic_counts.get(&issue.id) {
                format!(" ({}/{})", done, total).dimmed().to_string()
            } else {
                " (0)".dimmed().to_string()
            }
        } else {
            String::new()
        };

        let type_badge = format!("[{}]", type_str);
        let status_badge = format!("[{}]", status_str);
        println!(
            "{} {} {} {}{}",
            id_short.dimmed(),
            type_badge,
            status_badge,
            issue.title.bold(),
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

pub fn print_epic_view(epic: &Issue, child_issues: &[Issue]) {
    let status_str = match epic.status {
        Status::Proposed => "PROPOSED".white(),
        Status::Todo => "TODO".yellow(),
        Status::InProgress => "IN PROGRESS".blue(),
        Status::Done => "DONE".green(),
        Status::Blocked => "BLOCKED".red(),
    };

    println!("{}", "━".repeat(60).dimmed());
    println!("{}: {}", "Epic".bold(), epic.title.bold());
    println!("{}: {}", "ID".bold(), epic.short_id().cyan());
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
                issue.short_id().dimmed(),
                type_badge,
                status_badge,
                issue.title
            );
        }
    }

    println!();
}

pub fn print_issue_tree(issues: &[Issue]) {
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

    // Build epic counts
    let mut epic_counts: HashMap<Uuid, (usize, usize)> = HashMap::new();
    for issue in &non_epics {
        for rel in &issue.relationships {
            if rel.relation_type == crate::models::RelationType::PartOf {
                let entry = epic_counts.entry(rel.target_id).or_insert((0, 0));
                entry.0 += 1;
                if issue.status == Status::Done {
                    entry.1 += 1;
                }
            }
        }
    }

    // Display epics with their children
    for epic in &epics {
        let status_str = match epic.status {
            Status::Proposed => "PROPOSED".white(),
            Status::Todo => "TODO".yellow(),
            Status::InProgress => "IN PROGRESS".blue(),
            Status::Done => "DONE".green(),
            Status::Blocked => "BLOCKED".red(),
        };

        let count_str = if let Some((total, done)) = epic_counts.get(&epic.id) {
            format!(" ({}/{})", done, total).dimmed().to_string()
        } else {
            " (0)".dimmed().to_string()
        };

        let status_badge = format!("[{}]", status_str);
        println!(
            "{} {} {} {}{}",
            epic.short_id().dimmed(),
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

        children.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

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
                child.short_id().dimmed(),
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
                issue.short_id().dimmed(),
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
