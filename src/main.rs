mod cli;
mod display;
mod export;
mod flow;
mod jj;
mod models;
mod operations;
mod projects;
mod storage;
mod tui;

use crate::models::{Issue, IssueType, Status};
use crate::storage::LoadWarning;
use anyhow::{bail, Context, Result};
use clap::Parser;
use cli::{Cli, Commands, MetadataCommand, TagCommand};
use display::{
    print_creation_tips, print_epic_view, print_issue_detail, print_issue_list, print_issue_tree,
    print_summary,
};
use operations::Operations;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::io::{BufRead, IsTerminal, Write};
use uuid::Uuid;

fn main() -> Result<()> {
    // Rust ignores SIGPIPE by default, which makes any piped consumer that
    // exits early (e.g. `pebbles list | head`) turn writes into panics.
    // Restore default handling: die by SIGPIPE like standard Unix tools.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

    let cli = Cli::parse();

    // r[no-color-flag] - precedence: --no-color > NO_COLOR/CLICOLOR > tty.
    // The pure decision is display::should_color; this shell reads the
    // environment once and routes every render path through the `colored`
    // crate's global override, so list/show/epic/summary/flow/tips all
    // honor it uniformly. CLICOLOR_FORCE=1 still forces color (folded into
    // the tty check), but NO_COLOR beats it here, per the NO_COLOR spec.
    let env_no_color = env::var_os("NO_COLOR").is_some();
    let clicolor_zero = env::var("CLICOLOR").map(|v| v == "0").unwrap_or(false);
    let is_tty = std::io::stdout().is_terminal()
        || env::var("CLICOLOR_FORCE")
            .map(|v| v != "0")
            .unwrap_or(false);
    let colorize = display::should_color(cli.no_color, env_no_color, clicolor_zero, is_tty);
    colored::control::set_override(colorize);

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
            // r[type-templates] - the type's template fields are prompted
            // only when stdin is a terminal (an interactive create). Piped
            // and scripted creates (stdin not a terminal) skip the fields
            // silently, so they never prompt, never block, and their
            // behavior is byte-identical to before.
            let template_answers = if std::io::stdin().is_terminal() {
                prompt_template_fields(&issue_type)?
            } else {
                BTreeMap::new()
            };
            let outcome = ops.create(title, description, issue_type, approved, template_answers)?;
            print_warnings(&outcome.warnings);
            let issue = outcome.value;
            let short_id = ops.appropriate_length_id(&issue.id)?;
            println!("Created {} {}", issue.issue_type, short_id);
            let related_titles = ops.get_related_titles(&issue);
            let (incoming, incoming_warnings) = ops.get_incoming_relationships(&issue.id)?;
            print_warnings(&incoming_warnings);
            print_issue_detail(&issue, &related_titles, &incoming);

            // Post-creation guidance (r[guidance-tips]): optional tips
            // shown when the project has epics to link into. Selection is
            // pure (operations::select_creation_tips); rendering here.
            let (all_issues, _) = ops.list(None, None)?;
            let epics: Vec<(Uuid, String)> = all_issues
                .iter()
                .filter(|candidate| candidate.issue_type == IssueType::Epic)
                .map(|epic| (epic.id, epic.title.clone()))
                .collect();
            let guidance =
                operations::select_creation_tips(&issue.issue_type, &issue.relationships, &epics);
            // r[impl appropriate-length-id] - shorten both the new issue's
            // id (already computed above) and the epic candidates.
            let id_prefixes = ops.id_prefixes()?;
            print_creation_tips(&guidance, &short_id, &id_prefixes);
        }

        Commands::List {
            status,
            issue_type,
            search,
            grep,
            tag,
            metadata,
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
            // Empty --tag values are an error (r[continuity]): a blank tag
            // is not a selector for anything.
            let tag_filter: Vec<String> = tag.iter().map(|t| t.trim().to_string()).collect();
            if tag_filter.iter().any(|t| t.is_empty()) {
                anyhow::bail!("Tag filter cannot be empty");
            }
            // r[metadata-filter-parse] - each --metadata value must be a
            // KEY=VALUE pair with a non-empty key (r[continuity]: a blank
            // key is not a selector for anything). Parsed BEFORE listing so
            // a bad filter fails the whole request atomically.
            let metadata_filter: Vec<(String, String)> = metadata
                .iter()
                .map(|pair| operations::parse_metadata_filter(pair))
                .collect::<Result<Vec<_>>>()?;
            // r[grep-query-parse] - the full-text query is parsed BEFORE
            // listing so a malformed query fails the whole request
            // atomically, like the other filters (r[metadata-filter-parse]).
            let grep_terms = match &grep {
                Some(query) => Some(operations::parse_text_query(query)?),
                None => None,
            };
            let filter_applied = status_filter.is_some()
                || type_filter.is_some()
                || !tag_filter.is_empty()
                || !metadata_filter.is_empty()
                || orphaned
                || search.is_some()
                || grep.is_some();
            let (mut issues, warnings) = ops.list(status_filter, type_filter)?;
            print_warnings(&warnings);

            if let Some(query) = search {
                issues = operations::filter_by_title_query(&issues, &query);
            }

            // r[grep-query-syntax] - --grep terms are ANDed, and the
            // filter composes with every other filter.
            if let Some(terms) = &grep_terms {
                issues = operations::filter_by_text_query(&issues, terms);
            }

            // r[tag-filter-and]
            if !tag_filter.is_empty() {
                issues = operations::filter_by_tags(&issues, &tag_filter);
            }

            // r[metadata-filter-and]
            if !metadata_filter.is_empty() {
                issues = operations::filter_by_metadata(&issues, &metadata_filter);
            }

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
                // r[blocker-indicators]
                let blocker_statuses = ops.blocker_status()?;
                print_issue_list(
                    &issues,
                    &full_epic_counts,
                    filter_applied,
                    &id_prefixes,
                    &blocker_statuses,
                );
            }
        }

        Commands::Show { id } => {
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
            print_warnings(&warnings);
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
            let (succeeded, failed) = run_bulk_batch(&uuids, &prefixes, bulk, |id, _short| {
                let outcome = ops.update_status(id, new_status)?;
                print_warnings(&outcome.warnings);
                if outcome.value {
                    let issue = ops.show(id)?;
                    if bulk {
                        println!("✓ Updated {}", bulk_row(&issue, &prefixes));
                    } else {
                        println!("✓ Updated issue status");
                        let related_titles = ops.get_related_titles(&issue);
                        let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                        print_warnings(&warnings);
                        print_issue_detail(&issue, &related_titles, &incoming);
                    }
                } else if !bulk {
                    println!("No changes: issue already has this status");
                }
                Ok(())
            })?;
            finish_bulk("Status update", succeeded, &failed, uuids.len());
        }

        Commands::Approve { id } => {
            let outcome = ops.approve(&id)?;
            print_warnings(&outcome.warnings);
            let issue = ops.show(&id)?;
            let related_titles = ops.get_related_titles(&issue);
            let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
            print_warnings(&warnings);
            println!("✓ Approved issue");
            print_issue_detail(&issue, &related_titles, &incoming);
        }

        Commands::Update {
            ids,
            title,
            description,
            append_description,
        } => {
            let ids = collect_id_inputs(&ids)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let bulk = uuids.len() > 1;
            let prefixes = ops.id_prefixes()?;
            let (succeeded, failed) = run_bulk_batch(&uuids, &prefixes, bulk, |id, _short| {
                let outcome = ops.update(
                    id,
                    title.clone(),
                    description.clone(),
                    append_description.clone(),
                )?;
                print_warnings(&outcome.warnings);
                if outcome.value {
                    let issue = ops.show(id)?;
                    if bulk {
                        println!("✓ Updated {}", bulk_row(&issue, &prefixes));
                    } else {
                        println!("✓ Updated issue");
                        let related_titles = ops.get_related_titles(&issue);
                        let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                        print_warnings(&warnings);
                        print_issue_detail(&issue, &related_titles, &incoming);
                    }
                } else if !bulk {
                    println!("No changes: nothing to update");
                }
                Ok(())
            })?;
            finish_bulk("Update", succeeded, &failed, uuids.len());
        }

        Commands::Delete {
            ids,
            delete_links_too,
        } => {
            let ids = collect_id_inputs(&ids)?;
            let uuids = resolve_to_uuids(&ops, &ids)?;
            let prefixes = ops.id_prefixes()?;
            let (succeeded, failed) =
                run_bulk_batch(&uuids, &prefixes, uuids.len() > 1, |id, _short| {
                    let outcome = ops.delete(id, delete_links_too)?;
                    print_warnings(&outcome.warnings);
                    println!("✓ Deleted issue {}", outcome.value);
                    Ok(())
                })?;
            finish_bulk("Delete", succeeded, &failed, uuids.len());
        }

        Commands::Doctor => {
            let outcome = ops.doctor()?;
            for line in operations::doctor_lines(outcome.total_issues, &outcome.diagnostics) {
                println!("{}", line);
            }
            if !outcome.diagnostics.is_empty() {
                // Report and exit non-zero (r[index-consistent]); no error
                // line: the report already states the divergence (same
                // exit-without-error pattern as finish_bulk).
                let mut stdout = std::io::stdout();
                let _ = stdout.flush();
                std::process::exit(1);
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
            let (succeeded, failed) = run_bulk_batch(&uuids, &prefixes, bulk, |id, short| {
                let outcome = ops.add_relationship(id, relation_type.clone(), target)?;
                print_warnings(&outcome.warnings);
                if bulk {
                    println!(
                        "✓ Added relationship: {} {} {}",
                        short, relation_type, target_short
                    );
                } else {
                    println!("✓ Added relationship");
                }
                Ok(())
            })?;
            finish_bulk("Link", succeeded, &failed, uuids.len());
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
            let (succeeded, failed) = run_bulk_batch(&uuids, &prefixes, bulk, |id, short| {
                let outcome = ops.remove_relationship(id, relation_type.clone(), target)?;
                print_warnings(&outcome.warnings);
                if bulk {
                    println!(
                        "✓ Removed relationship: {} {} {}",
                        short, relation_type, target_short
                    );
                } else {
                    println!("✓ Removed relationship");
                }
                Ok(())
            })?;
            finish_bulk("Unlink", succeeded, &failed, uuids.len());
        }

        Commands::Tag { command } => match command {
            TagCommand::Add { id, tags } => {
                let outcome = ops.add_tags(&id, &tags)?;
                print_warnings(&outcome.warnings);
                if !outcome.value.is_empty() {
                    let issue = ops.show(&id)?;
                    let related_titles = ops.get_related_titles(&issue);
                    let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                    print_warnings(&warnings);
                    println!("✓ Added tags: {}", outcome.value.join(", "));
                    print_issue_detail(&issue, &related_titles, &incoming);
                } else {
                    println!("No changes: issue already has these tags");
                }
            }
            TagCommand::Remove { id, tags } => {
                let outcome = ops.remove_tags(&id, &tags)?;
                print_warnings(&outcome.warnings);
                if !outcome.value.is_empty() {
                    let issue = ops.show(&id)?;
                    let related_titles = ops.get_related_titles(&issue);
                    let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                    print_warnings(&warnings);
                    println!("✓ Removed tags: {}", outcome.value.join(", "));
                    print_issue_detail(&issue, &related_titles, &incoming);
                } else {
                    println!("No changes: issue does not have these tags");
                }
            }
        },

        Commands::Metadata { command } => match command {
            MetadataCommand::Set { id, key, value } => {
                let outcome = ops.set_metadata(&id, &key, &value)?;
                print_warnings(&outcome.warnings);
                if outcome.value {
                    let issue = ops.show(&id)?;
                    let related_titles = ops.get_related_titles(&issue);
                    let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                    print_warnings(&warnings);
                    println!("✓ Set metadata: {} = {}", key, value);
                    print_issue_detail(&issue, &related_titles, &incoming);
                } else {
                    println!("No changes: metadata {} already has this value", key);
                }
            }
            MetadataCommand::Unset { id, key } => {
                let outcome = ops.unset_metadata(&id, &key)?;
                print_warnings(&outcome.warnings);
                if outcome.value {
                    let issue = ops.show(&id)?;
                    let related_titles = ops.get_related_titles(&issue);
                    let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                    print_warnings(&warnings);
                    println!("✓ Removed metadata: {}", key);
                    print_issue_detail(&issue, &related_titles, &incoming);
                } else {
                    println!("No changes: issue does not have metadata key {}", key);
                }
            }
            MetadataCommand::Get { id, key } => {
                let issue = ops.show(&id)?;
                match key {
                    Some(key) => {
                        let key = key.trim().to_string();
                        operations::validate_metadata_key(&key)?;
                        match issue.metadata.get(&key) {
                            Some(value) => println!("{}", value),
                            None => anyhow::bail!(
                                "No metadata key '{}' on issue {}",
                                key,
                                issue.short_id()
                            ),
                        }
                    }
                    None => {
                        // r[metadata-get-sorted] - deterministic output:
                        // HashMap iteration order is unspecified.
                        for (k, v) in operations::sorted_metadata(&issue) {
                            println!("{}: {}", k, v);
                        }
                    }
                }
            }
        },

        Commands::Epic { id } => {
            let (epic, child_issues, warnings) = ops.epic(&id)?;
            print_warnings(&warnings);
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            print_epic_view(&epic, &child_issues, &id_prefixes);
        }

        Commands::Next => {
            let (issues, warnings) = ops.next()?;
            print_warnings(&warnings);
            // r[impl epic-progress-counts] - next is a filtered view.
            let full_epic_counts = ops.epic_counts()?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            // r[blocker-indicators]
            let blocker_statuses = ops.blocker_status()?;
            print_issue_list(
                &issues,
                &full_epic_counts,
                true,
                &id_prefixes,
                &blocker_statuses,
            );
        }

        // r[impl review-command]
        Commands::Review => {
            let (issues, warnings) = ops.list(Some(Status::Proposed), None)?;
            print_warnings(&warnings);
            // r[impl epic-progress-counts] - review is a filtered view.
            let full_epic_counts = ops.epic_counts()?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            // r[blocker-indicators]
            let blocker_statuses = ops.blocker_status()?;
            print_issue_list(
                &issues,
                &full_epic_counts,
                true,
                &id_prefixes,
                &blocker_statuses,
            );
        }

        Commands::Convert { id, new_type } => {
            let new_type_parsed: crate::models::IssueType =
                new_type.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            let outcome = ops.convert(&id, new_type_parsed.clone())?;
            print_warnings(&outcome.warnings);
            if outcome.value {
                let issue = ops.show(&id)?;
                let related_titles = ops.get_related_titles(&issue);
                let (incoming, warnings) = ops.get_incoming_relationships(&issue.id)?;
                print_warnings(&warnings);
                println!("✓ Converted issue type");
                print_issue_detail(&issue, &related_titles, &incoming);
            } else {
                println!(
                    "Warning: issue is already a {}; no changes made",
                    new_type_parsed
                );
            }
        }

        Commands::Reindex => {
            let outcome = ops.reindex()?;
            if outcome.wrote {
                println!("✓ Rebuilt index: {} issue(s)", outcome.entries);
            } else {
                println!("✓ Index is up to date: {} issue(s)", outcome.entries);
            }
            for description in &outcome.unparsable {
                eprintln!(
                    "Warning: Skipped unparsable issue file {}; it is not indexed",
                    description
                );
            }
            for file_name in &outcome.duplicates {
                eprintln!(
                    "Warning: Skipped {}: duplicate id (the first occurrence wins; this file is not indexed)",
                    file_name
                );
            }
        }

        Commands::Tui => {
            tui::run()?;
        }

        Commands::Summary => {
            let (summary, warnings) = ops.summary()?;
            print_warnings(&warnings);
            print_summary(&summary);
        }

        Commands::Export { format, output } => {
            let (issues, warnings) = ops.list(None, None)?;
            print_warnings(&warnings);
            let content = match format {
                cli::ExportFormat::Json => export::export_json(&issues),
                cli::ExportFormat::Csv => export::export_csv(&issues),
                cli::ExportFormat::Markdown => export::export_markdown(&issues),
            };
            if let Some(path) = output {
                std::fs::write(&path, content)?;
                println!("✓ Exported {} issue(s) to {}", issues.len(), path);
            } else {
                print!("{}", content);
            }
        }

        Commands::Import { file } => {
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read export bundle: {}", file))?;
            let outcome = ops.import_bundle(&content, &file)?;
            print_warnings(&outcome.warnings);
            println!("✓ Imported {} issue(s) from {}", outcome.value, file);
        }

        Commands::Flow {
            format,
            output,
            show_done,
            goal,
        } => {
            if format != cli::FlowFormat::Text && output.is_none() {
                anyhow::bail!(
                    "--output <path> is required for the '{}' format (only text writes to stdout)",
                    format
                );
            }

            // --goal threads the resolved goal issue into the renderers;
            // the critical-chain highlighting itself is loan e4937f0b.
            let goal = match &goal {
                Some(id) => Some(ops.resolve_ids(&[id])?[0]),
                None => None,
            };

            // Load all issues
            let (mut issues, warnings) = ops.list(None, None)?;
            print_warnings(&warnings);

            // Completed issues are hidden unless --show-done is given.
            if !show_done {
                issues.retain(|issue| issue.status != Status::Done);
            }

            // Validate goal (if any) has a path to roots
            if let Some(g) = goal {
                flow::critical_issues(&issues, g).map_err(|e| anyhow::anyhow!(e))?;
            }

            // Compute layout
            let layout = flow::compute_flow_layout(&issues)?;
            // r[impl appropriate-length-id]
            let id_prefixes = ops.id_prefixes()?;
            let content = match format {
                cli::FlowFormat::Text => flow::render_text(&issues, &layout, &id_prefixes, goal),
                cli::FlowFormat::Svg => flow::render_svg(&issues, &layout, &id_prefixes, goal),
                cli::FlowFormat::Dot => flow::render_dot(&issues, &layout, &id_prefixes, goal),
                cli::FlowFormat::Json => flow::render_json(&issues, &layout, &id_prefixes, goal),
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

/// Print load and mutation warnings to stderr in the CLI's own convention
/// (r[load-warnings-as-data]): one `Warning:` line per item. The storage and
/// operations layers only produce the warnings as data; rendering is the
/// shell's job so the TUI never sees raw stderr from a load.
fn print_warnings(warnings: &[LoadWarning]) {
    for warning in warnings {
        eprintln!("{}", warning.cli_line());
    }
}

/// Prompt for an issue type's template fields on an interactive terminal
/// and return the answers keyed by metadata key (r[type-templates]). Only
/// called when stdin is a terminal: the `new` flow gates the call on
/// `stdin().is_terminal()` so piped and scripted creates never reach the
/// prompts. A blank answer (bare Enter, or EOF) is skipped, so every
/// template field is optional.
fn prompt_template_fields(issue_type: &IssueType) -> Result<BTreeMap<String, String>> {
    let fields = operations::template_fields_for(issue_type.clone());
    let mut answers: BTreeMap<String, String> = BTreeMap::new();
    if fields.is_empty() {
        return Ok(answers);
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for field in fields {
        write!(out, "{} (blank to skip): ", field.label)?;
        out.flush()?;
        let mut line = String::new();
        input.read_line(&mut line)?;
        let answer = line.trim();
        if !answer.is_empty() {
            answers.insert(field.key, answer.to_string());
        }
    }
    Ok(answers)
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

/// Apply a per-issue operation to a resolved batch, fail-soft in bulk mode.
///
/// `apply` receives each issue's full UUID and its display short ID and
/// returns `Ok(())` when the operation succeeded. A no-op result (e.g. the
/// issue already had the requested status) is also `Ok(())` and prints
/// nothing in bulk mode, exactly as on the happy path. In bulk mode a
/// per-issue error is collected as `(short ID, reason)` and the batch
/// continues with the next issue; in single-issue mode the first error
/// propagates to the caller unchanged, preserving per-issue behavior.
fn run_bulk_batch<F>(
    uuids: &[Uuid],
    prefixes: &HashMap<Uuid, String>,
    bulk: bool,
    mut apply: F,
) -> Result<(usize, Vec<(String, String)>)>
where
    F: FnMut(&str, &str) -> Result<()>,
{
    let mut succeeded = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for uuid in uuids {
        let id = uuid.to_string();
        let short = display_uuid(uuid, prefixes);
        match apply(&id, &short) {
            Ok(()) => succeeded += 1,
            Err(err) => {
                if bulk {
                    failed.push((short, format!("{err:?}")));
                } else {
                    return Err(err);
                }
            }
        }
    }
    Ok((succeeded, failed))
}

/// Build the failure summary lines for a finished bulk batch: one line per
/// failed ID (operation, ID, reason), the list of IDs that were NOT
/// applied, and a final `N/M succeeded` line. Empty when nothing failed, so
/// the happy path stays minimal (one compact line per changed issue, no
/// summary noise).
fn bulk_failure_lines(
    operation: &str,
    succeeded: usize,
    failed: &[(String, String)],
    total: usize,
) -> Vec<String> {
    if failed.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = failed
        .iter()
        .map(|(id, reason)| format!("{} failed for {}: {}", operation, id, reason))
        .collect();
    let not_applied: Vec<&str> = failed.iter().map(|(id, _)| id.as_str()).collect();
    lines.push(format!("Not applied: {}", not_applied.join(", ")));
    lines.push(format!("{}/{} succeeded", succeeded, total));
    lines
}

/// Finish a bulk batch: on the happy path (nothing failed) this does
/// nothing. Otherwise it prints the failure summary to stderr (the applied
/// compact lines were already printed to stdout during the loop), flushes
/// stdout, and exits non-zero.
///
/// Exits instead of returning an error so the per-ID summary is not
/// followed by an `Error:` line: the failures are already reported above.
fn finish_bulk(operation: &str, succeeded: usize, failed: &[(String, String)], total: usize) {
    let lines = bulk_failure_lines(operation, succeeded, failed, total);
    if lines.is_empty() {
        return;
    }
    for line in lines {
        eprintln!("{}", line);
    }
    let mut stdout = std::io::stdout();
    let _ = stdout.flush();
    std::process::exit(1);
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
