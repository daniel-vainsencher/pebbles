# Pebbles

A rock-solid issue management tool built on top of [Jujutsu (jj)](https://github.com/martinvonz/jj).

## Why Pebbles?

Pebbles is designed to be:

- **Genuinely helpful** for both AI agents and humans working on projects
- **Stable as a rock** with a simple, reliable data model that causes zero distractions
- **Built on jj** taking full advantage of Jujutsu's powerful version control capabilities

Unlike traditional issue trackers that mix sqlite, jsonl, and git in fragile ways, Pebbles keeps it simple:
- Issues are stored as individual JSON files with meaningful names: `bug-a1b2c3d4-login-broken.json`
- An index tracks metadata in `.beads/index.json`
- Everything is versioned naturally by jj
- No database drama, no complex migrations, no data corruption

## Features

### Core
- ✅ Simple CRUD operations for issues
- ✅ Multiple issue types (bug, ux_problem, usecase, design, validation, epic, milestone)
- ✅ Status tracking (todo, in_progress, blocked, done)
- ✅ Relationship management (part_of, required_for, scheduled_before)
- ✅ Meaningful file names for better jj diffs
- ✅ Automatic jj integration with descriptive commit messages
- ✅ Short ID support (use first 4+ chars instead of full UUID)
- ✅ Filtering by status and type
- ✅ Beautiful colored terminal output
- ✅ Zero configuration needed

### Advanced (Planned)
- 🔄 Terminal UI (TUI) with interactive views
- 🔄 Fuzzy search and semantic matching
- 🔄 Bulk operations
- 🔄 Graphical relationship views
- 🔄 Custom metadata and templates

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (for building)
- [Jujutsu](https://github.com/martinvonz/jj) (jj CLI)

### Build from source

```bash
git clone <repository-url>
cd pebbles
cargo build --release
cargo install --path .
```

The binary will be installed to `~/.cargo/bin/pebbles`.

## Usage

### Initialize pebbles in a repository

```bash
pebbles init
```

This will:
- Initialize a jj repository (if not already present)
- Create the `.beads/` directory structure
- Set up the issue index

### Create a new issue

```bash
# Default type is 'bug'
pebbles new "Login button broken" -d "Button doesn't respond on mobile"

# Specify issue type
pebbles new "Dashboard redesign" --issue-type ux
pebbles new "User can reset password" --issue-type usecase
pebbles new "Authentication Epic" --issue-type epic
```

**Issue Types:**
- `bug` - Software defects
- `ux` (or `ux_problem`) - UX/UI issues
- `usecase` - User stories
- `design` - Architecture/design problems
- `validation` - Validation features
- `epic` - Feature grouping container
- `milestone` - Project milestones

### List all issues

```bash
pebbles list
```

Filter by status or type:

```bash
pebbles list --status todo
pebbles list --status in_progress
pebbles list --status blocked
pebbles list --issue-type bug
pebbles list --status in_progress --issue-type epic
```

### View issue details

```bash
pebbles show <id>
```

You can use the full UUID or just the first few characters:

```bash
pebbles show 53ed9aa8
```

### Update issue status

```bash
pebbles status <id> <new-status>
```

Examples:

```bash
pebbles status 53ed todo
pebbles status 53ed in_progress
pebbles status 53ed blocked
pebbles status 53ed done
```

### Link issues (relationships)

```bash
# Make a usecase part of an epic
pebbles link <usecase-id> part_of <epic-id>

# Mark dependencies
pebbles link <issue-id> required_for <other-id>

# Define ordering
pebbles link <issue-id> scheduled_before <other-id>

# Remove a relationship
pebbles unlink <issue-id> part_of <epic-id>
```

**Relationship Types:**
- `part_of` - Issue is part of an epic (validated - only bugs, ux, usecases, designs, validations allowed)
- `required_for` - This issue must be done before another
- `scheduled_before` - Ordering constraint for milestones

### Update issue details

```bash
pebbles update <id> --title "New title"
pebbles update <id> --description "New description"
pebbles update <id> --title "New title" --description "New description"
```

### Delete an issue

```bash
pebbles delete <id>
```

## Quick Example

```bash
# Initialize
pebbles init

# Create an epic for a feature
pebbles new "User Authentication" --issue-type epic
# Created: epic-a1b2c3d4

# Break it down
pebbles new "Login form design" --issue-type ux
# Created: ux-e5f6g7h8

pebbles new "User can login with email" --issue-type usecase
# Created: uc-i9j0k1l2

# Link them
pebbles link e5f6 part_of a1b2
pebbles link i9j0 part_of a1b2

# Work on it
pebbles status e5f6 in_progress

# Complete it
pebbles status e5f6 done

# Check progress
pebbles list --issue-type epic
pebbles show a1b2  # Shows relationships
```

## Architecture

Pebbles has a clean, simple architecture:

```
pebbles/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── cli.rs           # Command-line interface definitions
│   ├── models.rs        # Data structures (Issue, Status, Index)
│   ├── storage.rs       # File I/O operations
│   ├── jj.rs            # Jujutsu integration
│   ├── operations.rs    # Business logic (CRUD operations)
│   └── display.rs       # Pretty-printing for terminal output
└── Cargo.toml

.beads/
├── index.json                                      # Issue metadata (id → filename)
└── issues/
    ├── bug-a1b2c3d4-login-button-broken.json       # Bugs with 'bug-' prefix
    ├── ux-e5f6g7h8-dashboard-redesign.json         # UX issues with 'ux-' prefix
    ├── epic-i9j0k1l2-user-authentication.json      # Epics with 'epic-' prefix
    └── uc-m3n4o5p6-user-can-login.json             # Usecases with 'uc-' prefix
```

**Benefits of meaningful filenames:**
- Human-readable diffs in `jj log`
- Easy to find issues by browsing files
- Natural grouping by type
- Grep-friendly for searching

### Data Model

Each issue is stored as a JSON file:

```json
{
  "id": "53ed9aa8-b717-4d38-9760-c14bd4dbe115",
  "title": "Login button broken",
  "description": "Button doesn't respond on mobile",
  "status": "todo",
  "issue_type": "Bug",
  "tags": [],
  "relationships": [
    {
      "relation_type": "RequiredFor",
      "target_id": "417dce65-77d1-4f52-9718-39a4c8445f69"
    }
  ],
  "metadata": {},
  "created_at": "2026-01-06T03:08:07.123456Z",
  "updated_at": "2026-01-06T03:10:31.789012Z"
}
```

## Why Jujutsu?

Jujutsu (jj) is a next-generation version control system that offers several advantages over git:

1. **Every change is a commit** - No more staging area confusion
2. **Better conflict resolution** - Conflicts are first-class citizens
3. **Powerful history rewriting** - Safe and easy to reorganize commits
4. **Works with Git repos** - Can coexist with git remotes

For issue tracking, this means:
- Every issue change is automatically versioned
- Easy to sync issues across machines
- No special merge logic needed
- Full history of all changes

## Development

### Run tests

```bash
cargo test
```

### Run locally without installing

```bash
cargo run -- init
cargo run -- new "Test issue"
cargo run -- list
```

### Code structure

- `models.rs` - Core data structures
- `storage.rs` - File operations (isolated for easy testing)
- `jj.rs` - Jujutsu integration (calls jj CLI)
- `operations.rs` - Business logic (uses storage + jj)
- `cli.rs` - Command definitions
- `display.rs` - Terminal output formatting
- `main.rs` - Wires everything together

## Future Enhancements

Potential features for future versions:

- [ ] Tag management
- [ ] Issue dependencies and blocking relationships
- [ ] Comments on issues
- [ ] Issue templates
- [ ] Search functionality
- [ ] Issue import/export
- [ ] Web UI
- [ ] Git hooks integration
- [ ] Multi-repository project tracking

## Contributing

Contributions are welcome! Please ensure:

1. Code builds without warnings
2. All tests pass
3. New features include tests
4. Documentation is updated

## License

MIT

## Credits

Built with:
- [Rust](https://www.rust-lang.org/)
- [Jujutsu](https://github.com/martinvonz/jj)
- [clap](https://github.com/clap-rs/clap)
- [serde](https://serde.rs/)
- [colored](https://github.com/colored-rs/colored)
