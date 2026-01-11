mod cli;
mod display;
mod jj;
mod models;
mod operations;
mod storage;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use display::{print_epic_view, print_issue_detail, print_issue_list, print_issue_tree, print_summary};
use operations::Operations;
use std::env;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let current_dir = env::current_dir()?;
    let ops = Operations::new(&current_dir)?;

    match cli.command {
        Commands::Init => {
            ops.init()?;
        }

        Commands::New { title, description, issue_type } => {
            let issue_type = issue_type.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let issue = ops.create(title, description, issue_type)?;
            println!("✓ Created issue {}", issue.short_id());
            let related_titles = ops.get_related_titles(&issue);
            print_issue_detail(&issue, &related_titles);
        }

        Commands::List { status, issue_type, tree, orphaned } => {
            let status_filter = if let Some(s) = status {
                Some(s.parse().map_err(|e: String| anyhow::anyhow!(e))?)
            } else {
                None
            };
            let type_filter = if let Some(t) = issue_type {
                Some(t.parse().map_err(|e: String| anyhow::anyhow!(e))?)
            } else {
                None
            };
            let mut issues = ops.list(status_filter, type_filter)?;

            if orphaned {
                issues.retain(|issue| {
                    issue.issue_type.can_be_part_of_epic()
                        && !issue.relationships.iter().any(|rel| {
                            rel.relation_type == crate::models::RelationType::PartOf
                        })
                });
            }

            if tree {
                print_issue_tree(&issues);
            } else {
                print_issue_list(&issues);
            }
        }

        Commands::Show { id } => {
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            print_issue_detail(&issue, &related_titles);
        }

        Commands::Status { id, status } => {
            let new_status = status.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.update_status(&id, new_status)?;
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            println!("✓ Updated issue status");
            print_issue_detail(&issue, &related_titles);
        }

        Commands::Update { id, title, description } => {
            ops.update(&id, title, description)?;
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            println!("✓ Updated issue");
            print_issue_detail(&issue, &related_titles);
        }

        Commands::Delete { id } => {
            ops.delete(&id)?;
            println!("✓ Deleted issue {}", id);
        }

        Commands::Link { source, relation, target } => {
            let relation_type = relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.add_relationship(&source, relation_type, &target)?;
            println!("✓ Added relationship");
        }

        Commands::Unlink { source, relation, target } => {
            let relation_type = relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.remove_relationship(&source, relation_type, &target)?;
            println!("✓ Removed relationship");
        }

        Commands::Epic { id } => {
            let (epic, child_issues) = ops.epic(&id)?;
            print_epic_view(&epic, &child_issues);
        }

        Commands::Next => {
            let issues = ops.next()?;
            print_issue_list(&issues);
        }

        Commands::Convert { id, new_type } => {
            let new_type_parsed = new_type.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.convert(&id, new_type_parsed)?;
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            println!("✓ Converted issue type");
            print_issue_detail(&issue, &related_titles);
        }

        Commands::Summary => {
            let summary = ops.summary()?;
            print_summary(&summary);
        }
    }

    Ok(())
}
