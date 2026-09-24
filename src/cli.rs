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
    Edit files under .pebbles/ manually only if the CLI is failing. Afterwards, \
    run 'pebbles reindex' to rebuild the index from the issue files, then \
    'pebbles list' to verify that every issue still loads."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Initialize pebbles in the current repository")]
    Init {
        #[arg(
            long,
            help = "Initialize anyway when issue files exist but index.json is missing (orphans them into a fresh empty index)"
        )]
        force: bool,
    },

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
            help = "Issue type: bug - software defects requiring fixes\n  ux_problem - user experience issues\n  usecase - user stories or requirements\n  design - software design or architecture problems\n  validation - validation features or checks\n  epic - container for grouping related issues\n  milestone - ordering or prioritization marker"
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

        #[arg(
            short = 't',
            long,
            help = "Filter by type: bug - software defects requiring fixes\n  ux_problem - user experience issues\n  usecase - user stories or requirements\n  design - software design or architecture problems\n  validation - validation features or checks\n  epic - container for grouping related issues\n  milestone - ordering or prioritization marker"
        )]
        issue_type: Option<String>,

        #[arg(
            long,
            help = "Fuzzy-match issue titles against this query (case-insensitive; composes with --status, --issue-type, --tag, --metadata, and --grep)"
        )]
        search: Option<String>,

        #[arg(
            long,
            value_name = "QUERY",
            help = "Full-text search over titles and descriptions: space-separated terms are ANDed (each a case-insensitive substring), double-quoted phrases match verbatim. Composes with --status, --issue-type, --search, --tag, and --metadata."
        )]
        grep: Option<String>,

        #[arg(
            long,
            value_name = "TAG",
            action = clap::ArgAction::Append,
            help = "Keep issues carrying this tag (exact, case-sensitive). Repeatable: every given tag must be present (AND). Composes with --status, --issue-type, --search, --metadata, and --grep."
        )]
        tag: Vec<String>,

        #[arg(
            long,
            value_name = "KEY=VALUE",
            action = clap::ArgAction::Append,
            help = "Keep issues whose metadata carries this key=value pair (exact, case-sensitive; split at the first '='). Repeatable: every given pair must be present (AND). Composes with --status, --issue-type, --search, --tag, and --grep."
        )]
        metadata: Vec<String>,

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

    #[command(
        about = "Update status of one or more issues (multi-ID or stdin); per-ID failure summary (N/M succeeded), non-zero exit on any failure"
    )]
    Status {
        #[arg(
            value_name = "ID or STATUS",
            num_args = 1..,
            required = true,
            help = "Issue ID(s) (full UUID or prefix), followed by the new status as the last argument. When only a status is given, issue IDs are read from stdin (one per line, as printed by 'pebbles list')."
        )]
        args: Vec<String>,
    },

    #[command(about = "Approve a proposed issue (status proposed -> todo)")]
    Approve {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,
    },

    #[command(
        about = "Update one or more issues (multi-ID or stdin); per-ID failure summary (N/M succeeded), non-zero exit on any failure"
    )]
    Update {
        #[arg(
            value_name = "ID",
            num_args = 0..,
            help = "Issue ID(s) (full UUID or prefix). When none are given, IDs are read from stdin (one per line, as printed by 'pebbles list')."
        )]
        ids: Vec<String>,

        #[arg(short, long, help = "New title")]
        title: Option<String>,

        #[arg(short, long, help = "New description")]
        description: Option<String>,
    },

    #[command(
        about = "Delete one or more issues (multi-ID or stdin); per-ID failure summary (N/M succeeded), non-zero exit on any failure"
    )]
    Delete {
        #[arg(
            value_name = "ID",
            num_args = 0..,
            help = "Issue ID(s) (full UUID or prefix). When none are given, IDs are read from stdin (one per line, as printed by 'pebbles list')."
        )]
        ids: Vec<String>,

        #[arg(
            long,
            help = "Also remove relationships pointing at this issue from other issues"
        )]
        delete_links_too: bool,
    },

    #[command(about = "Check the store for index/issue divergence (read-only)")]
    Doctor,

    #[command(
        about = "Add a relationship (multi-source or stdin); per-ID failure summary (N/M succeeded), non-zero exit on any failure"
    )]
    Link {
        #[arg(
            value_name = "SOURCE RELATION TARGET",
            num_args = 2..,
            required = true,
            help = "Source issue ID(s), the relationship type, and the target issue ID. The last two arguments are the relationship type (part_of, required_for, scheduled_before) and the target; everything before them is a source ID. When only RELATION and TARGET are given, source IDs are read from stdin."
        )]
        args: Vec<String>,
    },

    #[command(
        about = "Remove a relationship (multi-source or stdin); per-ID failure summary (N/M succeeded), non-zero exit on any failure"
    )]
    Unlink {
        #[arg(
            value_name = "SOURCE RELATION TARGET",
            num_args = 2..,
            required = true,
            help = "Source issue ID(s), the relationship type, and the target issue ID. The last two arguments are the relationship type (part_of, required_for, scheduled_before) and the target; everything before them is a source ID. When only RELATION and TARGET are given, source IDs are read from stdin."
        )]
        args: Vec<String>,
    },

    #[command(about = "Manage an issue's tags (add or remove; per-issue, non-bulk)")]
    Tag {
        #[command(subcommand)]
        command: TagCommand,
    },

    #[command(about = "Manage an issue's metadata (set, unset, or get; per-issue, non-bulk)")]
    Metadata {
        #[command(subcommand)]
        command: MetadataCommand,
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
            help = "New issue type: bug - software defects requiring fixes\n  ux_problem - user experience issues\n  usecase - user stories or requirements\n  design - software design or architecture problems\n  validation - validation features or checks\n  epic - container for grouping related issues\n  milestone - ordering or prioritization marker"
        )]
        new_type: String,
    },

    #[command(about = "Interactive TUI")]
    Tui,

    #[command(
        about = "Rebuild .pebbles/index.json from the issue files (repair command)",
        long_about = "Rebuild .pebbles/index.json from the issue files under .pebbles/issues/.\n\n        The sanctioned repair for a conflicted or desynchronized index (e.g. after a merge of two\n        .pebbles copies). The rebuilt index contains every issue file with a parseable UUID id\n        (first file wins per id); entry order is preserved from the current index when it parses,\n        and is deterministic otherwise. Unparsable files and duplicate ids are reported. The\n        rebuilt index is written atomically, verified with a load, and left in the working copy\n        for you to commit."
    )]
    Reindex,

    #[command(about = "Show project summary and statistics")]
    Summary,

    #[command(about = "Export all issues as a JSON bundle, CSV, or Markdown document")]
    Export {
        #[arg(value_enum, help = "Output format (json, csv, markdown)")]
        format: ExportFormat,

        #[arg(short = 'o', long, help = "Output file path (default: stdout)")]
        output: Option<String>,
    },

    #[command(
        about = "Import issues from a pebbles export JSON bundle (additive; rejects clashes)",
        long_about = "Import issues from a pebbles export JSON bundle written by 'pebbles export json'.\n\n        The bundle is fully validated before anything is written: the envelope must be a pebbles export\n        bundle (format pebbles-export, version 1), every issue must pass the same schema checks as a\n        loaded issue file, duplicate ids inside the bundle are rejected, and an id that already exists\n        in this store is an error (import is additive and never overwrites). Relationships whose target\n        is neither in the bundle nor in this store are dropped with a warning. All imported issues are\n        written and committed to jj in one operation."
    )]
    Import {
        #[arg(help = "Path to a pebbles export JSON bundle")]
        file: String,
    },

    #[command(about = "Visualize project flow (EXPERIMENTAL)")]
    Flow {
        #[arg(
            long,
            value_enum,
            default_value_t = FlowFormat::Text,
            help = "Output format (text, svg, json, dot)"
        )]
        format: FlowFormat,

        #[arg(
            short,
            long,
            help = "Output file path (required for svg, json, and dot; text defaults to stdout)"
        )]
        output: Option<String>,

        #[arg(long, help = "Include completed issues (default: hide them)")]
        show_done: bool,

        #[arg(
            long,
            value_name = "ID",
            help = "Highlight the critical chain to this goal issue (full UUID or prefix)"
        )]
        goal: Option<String>,
    },
}

/// Subcommands of `pebbles tag`: add and remove tags on a single issue.
///
/// Both commands accept one or more tags after the issue ID (`tag add <id>
/// <tag...>`). Tags are matched and stored exactly as given (trimmed, but
/// case-sensitive); `add` appends and deduplicates, `remove` deletes every
/// matching occurrence.
#[derive(Subcommand)]
pub enum TagCommand {
    #[command(about = "Add tags to an issue (appends and deduplicates)")]
    Add {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(
            value_name = "TAG",
            num_args = 1..,
            required = true,
            help = "Tag(s) to add"
        )]
        tags: Vec<String>,
    },

    #[command(about = "Remove tags from an issue")]
    Remove {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(
            value_name = "TAG",
            num_args = 1..,
            required = true,
            help = "Tag(s) to remove"
        )]
        tags: Vec<String>,
    },
}

/// Subcommands of `pebbles metadata`: set, unset, and get metadata fields
/// on a single issue.
///
/// Metadata is a free-form `string -> string` map; keys and values are
/// trimmed on entry, keys must be non-empty, and values may be empty. `set`
/// upserts one key (replacing an existing value), `unset` removes one key (a
/// no-op when absent), and `get` prints all pairs or a single value. Keys are
/// matched exactly and case-sensitively (r[metadata-set-upsert]).
#[derive(Subcommand)]
pub enum MetadataCommand {
    #[command(about = "Set a metadata key on an issue (upserts: replaces an existing value)")]
    Set {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(value_name = "KEY", help = "Metadata key (trimmed; must be non-empty)")]
        key: String,

        #[arg(value_name = "VALUE", help = "Metadata value (trimmed; may be empty)")]
        value: String,
    },

    #[command(about = "Remove a metadata key from an issue (no-op when absent)")]
    Unset {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(value_name = "KEY", help = "Metadata key (trimmed; must be non-empty)")]
        key: String,
    },

    #[command(about = "Get an issue's metadata (all keys; or one key's value)")]
    Get {
        #[arg(help = "Issue ID (full UUID or prefix)")]
        id: String,

        #[arg(value_name = "KEY", help = "Metadata key (all keys when omitted)")]
        key: Option<String>,
    },
}

/// Output format for `pebbles export` (r[export-bundle], r[csv-schema],
/// r[export-markdown]): `json` writes the versioned bundle that `pebbles
/// import` accepts; `csv` writes one flat RFC-4180 row per issue; and
/// `markdown` writes a readable per-epic + orphans document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    Json,
    Csv,
    Markdown,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ExportFormat::Json => "json",
            ExportFormat::Csv => "csv",
            ExportFormat::Markdown => "markdown",
        };
        write!(f, "{}", name)
    }
}

/// Output format for `pebbles flow` (r[flow-format]). `json` and `dot` are
/// accepted by the CLI today but their backends are separate loans
/// (e7dd87ca for JSON, ea4b09f2 for DOT); the current binary rejects them
/// with a clear not-implemented error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum FlowFormat {
    Text,
    Svg,
    Json,
    Dot,
}

impl std::fmt::Display for FlowFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            FlowFormat::Text => "text",
            FlowFormat::Svg => "svg",
            FlowFormat::Json => "json",
            FlowFormat::Dot => "dot",
        };
        write!(f, "{}", name)
    }
}
