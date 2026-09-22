mod cli;
mod display;
mod flow;
mod jj;
mod models;
mod operations;
mod storage;
mod projects;
mod tui;

use crate::models::Status;
use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use display::{
    print_epic_view, print_issue_detail, print_issue_list, print_issue_tree, print_summary,
};
use operations::Operations;
use std::env;

fn main() -> Result<()> {
    // Rust ignores SIGPIPE by default, which makes any piped consumer that
    // exits early (e.g. `pebbles list | head`) turn writes into panics.
    // Restore default handling: die by SIGPIPE like standard Unix tools.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let cli = Cli::parse();
    let current_dir = env::current_dir()?;
    let ops = Operations::new(&current_dir)?;

    match cli.command {
        Commands::Init { force } => {
            ops.init(force)?;
        }

        Commands::New {
            title,
            description,
            issue_type,
            approved,
        } => {
            let issue_type = issue_type.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let issue = ops.create(title, description, issue_type, approved)?;
            let short_id = ops.appropriate_length_id(&issue.id)?;
            println!("Created {} {}", issue.issue_type, short_id);
            let related_titles = ops.get_related_titles(&issue);
            let incoming = ops.get_incoming_relationships(&issue.id)?;
            print_issue_detail(&issue, &related_titles, &incoming);
        }

        Commands::List {
            status,
            issue_type,
            tree,
            orphaned,
        } => {
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
            let filter_applied = status_filter.is_some() || type_filter.is_some() || orphaned;
            let mut issues = ops.list(status_filter, type_filter)?;

            if orphaned {
                issues.retain(|issue| {
                    issue.issue_type.can_be_part_of_epic()
                        && !issue
                            .relationships
                            .iter()
                            .any(|rel| rel.relation_type == crate::models::RelationType::PartOf)
                });
            }

            // r[impl epic-progress-counts]
            let full_epic_counts = ops.epic_counts()?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            if tree {
                print_issue_tree(&issues, &full_epic_counts, filter_applied, &id_prefixes);
            } else {
                print_issue_list(&issues, &full_epic_counts, filter_applied, &id_prefixes);
            }
        }

        Commands::Show { id } => {
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            let incoming = ops.get_incoming_relationships(&issue.id)?;
            print_issue_detail(&issue, &related_titles, &incoming);
        }

        Commands::Status { id, status } => {
            let new_status = status.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            if ops.update_status(&id, new_status)? {
                let issue = ops.show(&id)?;
                let related_titles = ops.get_related_titles(&issue);
                let incoming = ops.get_incoming_relationships(&issue.id)?;
                println!("✓ Updated issue status");
                print_issue_detail(&issue, &related_titles, &incoming);
            } else {
                println!("No changes: issue already has this status");
            }
        }

        Commands::Approve { id } => {
            ops.approve(&id)?;
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            let incoming = ops.get_incoming_relationships(&issue.id)?;
            println!("✓ Approved issue");
            print_issue_detail(&issue, &related_titles, &incoming);
        }

        Commands::Update {
            id,
            title,
            description,
        } => {
            if ops.update(&id, title, description)? {
                let issue = ops.show(&id)?;
                let related_titles = ops.get_related_titles(&issue);
                let incoming = ops.get_incoming_relationships(&issue.id)?;
                println!("✓ Updated issue");
                print_issue_detail(&issue, &related_titles, &incoming);
            } else {
                println!("No changes: nothing to update");
            }
        }

        Commands::Delete {
            id,
            delete_links_too,
        } => {
            let short = ops.delete(&id, delete_links_too)?;
            println!("✓ Deleted issue {}", short);
        }

        Commands::Link {
            source,
            relation,
            target,
        } => {
            let relation_type = relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.add_relationship(&source, relation_type, &target)?;
            println!("✓ Added relationship");
        }

        Commands::Unlink {
            source,
            relation,
            target,
        } => {
            let relation_type = relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.remove_relationship(&source, relation_type, &target)?;
            println!("✓ Removed relationship");
        }

        Commands::Epic { id } => {
            let (epic, child_issues) = ops.epic(&id)?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            print_epic_view(&epic, &child_issues, &id_prefixes);
        }

        Commands::Next => {
            let issues = ops.next()?;
            // r[impl epic-progress-counts] - next is a filtered view.
            let full_epic_counts = ops.epic_counts()?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            print_issue_list(&issues, &full_epic_counts, true, &id_prefixes);
        }

        // r[impl review-command]
        Commands::Review => {
            let issues = ops.list(Some(Status::Proposed), None)?;
            // r[impl epic-progress-counts] - review is a filtered view.
            let full_epic_counts = ops.epic_counts()?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            print_issue_list(&issues, &full_epic_counts, true, &id_prefixes);
        }

        Commands::Convert { id, new_type } => {
            let new_type_parsed: crate::models::IssueType =
                new_type.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            if ops.convert(&id, new_type_parsed.clone())? {
                let issue = ops.show(&id)?;
                let related_titles = ops.get_related_titles(&issue);
                let incoming = ops.get_incoming_relationships(&issue.id)?;
                println!("✓ Converted issue type");
                print_issue_detail(&issue, &related_titles, &incoming);
            } else {
                println!(
                    "Warning: issue is already a {}; no changes made",
                    new_type_parsed
                );
            }
        }


        Commands::Tui => {
            tui::run()?;
        }

        Commands::Summary => {
            let summary = ops.summary()?;
            print_summary(&summary);
        }

        Commands::Flow { format, output } => {
            // Load all issues
            let issues = ops.list(None, None)?;

            // Compute layout
            let layout = flow::compute_flow_layout(&issues)?;

            // Determine output format
            let fmt = format.as_deref().unwrap_or("text");
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            let content = match fmt {
                "svg" => flow::render_svg(&issues, &layout, &id_prefixes),
                "text" => flow::render_text(&issues, &layout, &id_prefixes),
                other => anyhow::bail!("Invalid format '{}': expected 'text' or 'svg'", other),
            };

            // Output to file or stdout
            if let Some(path) = output {
                std::fs::write(&path, content)?;
                println!("✓ Flow visualization written to {}", path);
            } else {
                print!("{}", content);
            }
        }
    }

    Ok(())
}
