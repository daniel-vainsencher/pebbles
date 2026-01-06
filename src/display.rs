use colored::*;
use crate::models::{Issue, IssueType, RelationType, Status};

pub fn print_issue_list(issues: &[Issue]) {
    if issues.is_empty() {
        println!("{}", "No issues found.".dimmed());
        return;
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
        println!(
            "{} {} {} {}",
            id_short.dimmed(),
            format!("[{}]", type_str),
            format!("[{}]", status_str),
            issue.title.bold()
        );
    }
}

pub fn print_issue_detail(issue: &Issue) {
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
            println!("  {} {}", rel_str.dimmed(), rel.target_id.to_string()[..8].to_string().cyan());
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
