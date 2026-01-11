use colored::*;
use crate::models::{Issue, IssueType, RelationType, Status};
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

        println!(
            "{} {} {} {}{}",
            id_short.dimmed(),
            format!("[{}]", type_str),
            format!("[{}]", status_str),
            issue.title.bold(),
            count_str
        );
    }
}

pub fn print_issue_detail(issue: &Issue, related_titles: &HashMap<Uuid, String>) {
    let status_str = match issue.status {
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

    if !issue.metadata.is_empty() {
        println!("\n{}", "Metadata:".bold());
        for (key, value) in &issue.metadata {
            println!("  {}: {}", key.dimmed(), value);
        }
    }
    
    println!("\n{}: {}", "Created".bold(), issue.created_at.format("%Y-%m-%d %H:%M:%S"));
    println!("{}: {}", "Updated".bold(), issue.updated_at.format("%Y-%m-%d %H:%M:%S"));
    println!("{}: {}", "File".bold(), issue.file_name().dimmed());
    println!("{}", "━".repeat(60).dimmed());
}
