mod cli;
mod display;
mod jj;
mod models;
mod operations;
mod storage;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use display::{print_issue_detail, print_issue_list};
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
            print_issue_detail(&issue);
        }

        Commands::List { status, issue_type } => {
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
            let issues = ops.list(status_filter, type_filter)?;
            print_issue_list(&issues);
        }

        Commands::Show { id } => {
            let issue = ops.show(&id)?;
            print_issue_detail(&issue);
        }

        Commands::Status { id, status } => {
            let new_status = status.parse().map_err(|e: String| anyhow::anyhow!(e))?;
            ops.update_status(&id, new_status)?;
            let issue = ops.show(&id)?;
            println!("✓ Updated issue status");
            print_issue_detail(&issue);
        }

        Commands::Update { id, title, description } => {
            ops.update(&id, title, description)?;
            let issue = ops.show(&id)?;
            println!("✓ Updated issue");
            print_issue_detail(&issue);
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
    }

    Ok(())
}
