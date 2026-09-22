mod cli;
mod display;
mod flow;
mod jj;
mod models;
mod operations;
mod projects;
mod storage;
mod tui;

use crate::models::{Issue, Status};
use anyhow::{bail, Result};
use clap::Parser;
use cli::{Cli, Commands};
use display::{
    print_epic_view, print_issue_detail, print_issue_list, print_issue_tree, print_summary,
};
use operations::Operations;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, IsTerminal};
use uuid::Uuid;

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

        Commands::Status { args } => {
            // The last positional argument is the status; everything before
            // it is an issue ID. With only a status given, IDs come from
            // stdin (Unix composition: `pebbles list | pebbles status done`).
            let (status_token, explicit_ids) =
                args.split_last().expect("clap requires >= 1 argument");
            let new_status: Status = status_token.parse().map_err(|_e: String| {
                if explicit_ids.is_empty() {
                    anyhow::anyhow!(concat!(
                        "Missing status: expected a status (proposed, todo, in_progress, ",
                        "done, blocked) as the last argument. Example: 'pebbles status ",
                        "<id> <status>' or 'pebbles list --status todo | pebbles status ",
                        "in_progress'"
                    ))
                } else {
                    anyhow::anyhow!("Invalid status: {}", status_token)
                }
            })?;
            let ids = collect_id_inputs(explicit_ids)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let bulk = uuids.len() > 1;
            let prefixes = ops.id_prefixes()?;
            for uuid in &uuids {
                let id = uuid.to_string();
                if ops.update_status(&id, new_status)? {
                    let issue = ops.show(&id)?;
                    if bulk {
                        println!("✓ Updated {}", bulk_row(&issue, &prefixes));
                    } else {
                        println!("✓ Updated issue status");
                        let related_titles = ops.get_related_titles(&issue);
                        let incoming = ops.get_incoming_relationships(&issue.id)?;
                        print_issue_detail(&issue, &related_titles, &incoming);
                    }
                } else if !bulk {
                    println!("No changes: issue already has this status");
                }
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
            ids,
            title,
            description,
        } => {
            let ids = collect_id_inputs(&ids)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let bulk = uuids.len() > 1;
            let prefixes = ops.id_prefixes()?;
            for uuid in &uuids {
                let id = uuid.to_string();
                if ops.update(&id, title.clone(), description.clone())? {
                    let issue = ops.show(&id)?;
                    if bulk {
                        println!("✓ Updated {}", bulk_row(&issue, &prefixes));
                    } else {
                        println!("✓ Updated issue");
                        let related_titles = ops.get_related_titles(&issue);
                        let incoming = ops.get_incoming_relationships(&issue.id)?;
                        print_issue_detail(&issue, &related_titles, &incoming);
                    }
                } else if !bulk {
                    println!("No changes: nothing to update");
                }
            }
        }

        Commands::Delete {
            ids,
            delete_links_too,
        } => {
            let ids = collect_id_inputs(&ids)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            for uuid in &uuids {
                let short = ops.delete(&uuid.to_string(), delete_links_too)?;
                println!("✓ Deleted issue {}", short);
            }
        }

        Commands::Link { args } => {
            let (sources, relation, target) = split_source_args(&args);
            let relation_type: crate::models::RelationType =
                relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let ids = collect_id_inputs(&sources)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let bulk = uuids.len() > 1;
            let target_uuid = ops.resolve_ids(&[target])?[0];
            let prefixes = ops.id_prefixes()?;
            let target_short = display_uuid(&target_uuid, &prefixes);
            for uuid in &uuids {
                ops.add_relationship(&uuid.to_string(), relation_type.clone(), target)?;
                if bulk {
                    println!(
                        "✓ Added relationship: {} {} {}",
                        display_uuid(uuid, &prefixes),
                        relation_type,
                        target_short
                    );
                } else {
                    println!("✓ Added relationship");
                }
            }
        }

        Commands::Unlink { args } => {
            let (sources, relation, target) = split_source_args(&args);
            let relation_type: crate::models::RelationType =
                relation.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let ids = collect_id_inputs(&sources)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let bulk = uuids.len() > 1;
            let target_uuid = ops.resolve_ids(&[target])?[0];
            let prefixes = ops.id_prefixes()?;
            let target_short = display_uuid(&target_uuid, &prefixes);
            for uuid in &uuids {
                ops.remove_relationship(&uuid.to_string(), relation_type.clone(), target)?;
                if bulk {
                    println!(
                        "✓ Removed relationship: {} {} {}",
                        display_uuid(uuid, &prefixes),
                        relation_type,
                        target_short
                    );
                } else {
                    println!("✓ Removed relationship");
                }
            }
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

/// Collect issue ID inputs for a bulk command: the positional IDs, or —
/// when none were given — IDs read from stdin. Stdin input accepts the
/// output of a listing command (`pebbles list`, `next`, `review`, `epic`)
/// as well as bare IDs (see `operations::parse_id_input_line`). Errors
/// when nothing is available, without blocking on an interactive terminal.
fn collect_id_inputs(ids: &[String]) -> Result<Vec<String>> {
    if !ids.is_empty() {
        return Ok(ids.to_vec());
    }
    if std::io::stdin().is_terminal() {
        bail!(concat!(
            "No issue IDs given. Pass IDs as arguments or pipe them via stdin ",
            "(e.g. 'pebbles list --status todo | pebbles status in_progress')."
        ));
    }
    let stdin = std::io::stdin();
    let mut out = Vec::new();
    for line in stdin.lock().lines() {
        let line = line?;
        out.extend(
            operations::parse_id_input_line(&line)
                .into_iter()
                .map(String::from),
        );
    }
    if out.is_empty() {
        bail!("No issue IDs found on stdin.");
    }
    Ok(out)
}

/// Resolve a batch of ID inputs to UUIDs (order preserved, duplicates
/// dropped; fails fast on the first unresolvable input).
fn resolve_to_uuids(ops: &Operations, ids: &[String]) -> Result<Vec<Uuid>> {
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    ops.resolve_ids(&refs)
}

/// Split `link`/`unlink` positional args into (source IDs, relation,
/// target): the last two args are the relationship type and the target;
/// everything before them is a source ID. clap guarantees >= 2 args.
fn split_source_args(args: &[String]) -> (Vec<String>, &str, &str) {
    let (sources, tail) = args.split_at(args.len() - 2);
    (sources.to_vec(), &tail[0], &tail[1])
}

/// Appropriate-length ID for a UUID, looked up in a precomputed prefix map.
fn display_uuid(uuid: &Uuid, prefixes: &HashMap<Uuid, String>) -> String {
    prefixes
        .get(uuid)
        .cloned()
        .unwrap_or_else(|| uuid.to_string())
}

/// One-line issue summary in the `pebbles list` format
/// (`{short} [{type}] [{STATUS}] {title}`), used for bulk-operation output.
fn bulk_row(issue: &Issue, prefixes: &HashMap<Uuid, String>) -> String {
    format!(
        "{} [{}] [{}] {}",
        display_uuid(&issue.id, prefixes),
        issue.issue_type.short_code(),
        status_label(&issue.status),
        issue.title
    )
}

/// Uppercase human label for a status, matching `pebbles list` output.
fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Proposed => "PROPOSED",
        Status::Todo => "TODO",
        Status::InProgress => "IN PROGRESS",
        Status::Done => "DONE",
        Status::Blocked => "BLOCKED",
    }
}
