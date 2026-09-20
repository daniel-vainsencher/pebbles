use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pebbles")]
#[command(
    author,
    version,
    about = "A rock-solid issue management tool built on top of Jujutsu",
    long_about = "A rock-solid issue management tool built on top of Jujutsu\n\n\
    Issues are JSON files under .pebbles/. Make all changes through the pebbles \
    CLI: it validates relationships, renames files when titles or types change, \
    keeps the index in sync, and commits each operation to jj.\n\n\
    Edit files under .pebbles/ manually only if the CLI is failing (for example, \
    to repair a corrupt index). Afterwards, run 'pebbles list' to verify that \
    every issue still loads."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Initialize pebbles in the current repository")]
    Init,

    #[command(about = "Create a new issue")]
    New {
        #[arg(help = "Issue title")]
        title: String,

        #[arg(short, long, help = "Issue description")]
        description: Option<String>,

        #[arg(
            short = 't',
            long,
            default_value = "bug",
            help = "Issue type (bug, ux_problem, usecase, design, validation, epic, milestone)"
        )]
        issue_type: String,

        #[arg(
            long,
            help = "Mark the issue as approved (status todo) instead of proposed"
        )]
        approved: bool,
    },

    #[command(about = "List all issues")]
    List {
        #[arg(
            short,
            long,
            help = "Filter by status (proposed, todo, in_progress, done, blocked)"
        )]
        status: Option<String>,

        #[arg(short = 't', long, help = "Filter by type (bug, ux_problem, etc.)")]
        issue_type: Option<String>,

        #[arg(
            long,
            help = "Show hierarchical tree structure with epic relationships"
        )]
        tree: bool,

        #[arg(long, help = "Show only orphaned issues (not part of any epic)")]
        orphaned: bool,
    },

    #[command(about = "Show details of an issue")]
    Show {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,
    },

    #[command(about = "Update issue status")]
    Status {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(help = "New status (proposed, todo, in_progress, done, blocked)")]
        status: String,
    },

    #[command(about = "Approve a proposed issue (status proposed -> todo)")]
    Approve {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,
    },

    #[command(about = "Update issue details")]
    Update {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(short, long, help = "New title")]
        title: Option<String>,

        #[arg(short, long, help = "New description")]
        description: Option<String>,
    },

    #[command(about = "Delete an issue")]
    Delete {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(
            long,
            help = "Also remove relationships pointing at this issue from other issues"
        )]
        delete_links_too: bool,
    },

    #[command(about = "Add a relationship between issues")]
    Link {
        #[arg(help = "Source issue ID")]
        source: String,

        #[arg(help = "Relationship type (part_of, required_for, scheduled_before)")]
        relation: String,

        #[arg(help = "Target issue ID")]
        target: String,
    },

    #[command(about = "Remove a relationship between issues")]
    Unlink {
        #[arg(help = "Source issue ID")]
        source: String,

        #[arg(help = "Relationship type (part_of, required_for, scheduled_before)")]
        relation: String,

        #[arg(help = "Target issue ID")]
        target: String,
    },

    #[command(about = "Show an epic with all its child issues")]
    Epic {
        #[arg(help = "Epic ID (full UUID or prefix)")]
        id: String,
    },

    #[command(about = "Show actionable TODO issues with no blockers")]
    Next,

    #[command(about = "List proposed issues awaiting review")]
    Review,

    #[command(about = "Convert an issue to a different type")]
    Convert {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(
            help = "New issue type (bug, ux_problem, usecase, design, validation, epic, milestone)"
        )]
        new_type: String,
    },

    #[command(about = "Interactive TUI (experimental)")]
    Tui,

    #[command(about = "Show project summary and statistics")]
    Summary,

    #[command(about = "Visualize project flow (EXPERIMENTAL)")]
    Flow {
        #[arg(long, help = "Output format (text, svg)")]
        format: Option<String>,

        #[arg(short, long, help = "Output file path")]
        output: Option<String>,
    },
}
