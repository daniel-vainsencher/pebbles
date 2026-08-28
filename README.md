# Pebbles

A rock-solid issue management tool built on [Jujutsu (jj)](https://github.com/martinvonz/jj).

## What is Pebbles?

Pebbles is a command-line issue tracker designed for developers who want:

- **Version-controlled issues** - Issues are stored as JSON files tracked by jj
- **Offline-first** - Work without a server or internet connection
- **Simple and reliable** - No database, no complex state, no corruption
- **Developer-friendly** - Grep and diff your issues like code
- **AI agent compatible** - Structured data, clear semantics, predictable behavior

## Why Use Pebbles?

**Use Pebbles when you want to:**
- Track issues alongside your code in version control
- Work offline or in disconnected environments
- Have full control over your issue data
- Script and automate issue management
- Keep things simple and reliable

**Pebbles is ideal for:**
- Personal projects
- Small teams using jj for version control
- AI agents managing development tasks
- Projects where issues should be versioned with code

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.70+
- [Jujutsu (jj)](https://github.com/martinvonz/jj)

### Build from Source

```bash
git clone <repository-url>
cd pebbles
cargo build --release
cargo install --path .
```

The `pebbles` binary will be installed to `~/.cargo/bin/pebbles`.

## Quick Start

### Initialize

```bash
cd your-project
pebbles init
```

This initializes a jj repository (if needed) and creates the `.pebbles/` directory.

### Create Issues

```bash
# Create a bug (starts as proposed, awaiting review)
pebbles new "Login button broken" -d "Button doesn't respond on mobile"

# Create an issue that skips review and starts as todo
pebbles new "Hotfix" --approved

# Create with specific type
pebbles new "Dashboard redesign" --issue-type ux
pebbles new "User can export data" --issue-type usecase
pebbles new "Authentication Epic" --issue-type epic
```

**Issue types:** `bug`, `ux` (shorthand for `ux_problem`), `usecase`, `design`, `validation`, `epic`, `milestone`

### Review Proposed Issues

New issues start in `proposed` status until they are approved. See what is waiting for review, then approve or delete:

```bash
# See all proposed issues
pebbles review

# Approve one (moves it to todo)
pebbles status a1b2 todo

# Reject one
pebbles delete a1b2
```

### List and Filter

```bash
# List all issues
pebbles list

# Filter by status
pebbles list --status proposed
pebbles list --status todo
pebbles list --status in_progress

# Filter by type
pebbles list --issue-type bug

# Combine filters
pebbles list --status in_progress --issue-type epic
```

**Statuses:** `proposed`, `todo`, `in_progress`, `blocked`, `done`

### View Details

```bash
pebbles show <id>

# Short IDs work (minimum 3 characters)[^1]
pebbles show a1b
```

[^1]: ID length is at least 3 characters. As your project grows, you may need longer prefixes if short IDs become ambiguous.

### Update Issues

```bash
# Change status
pebbles status a1b2 in_progress  # e.g., approving a proposed issue
pebbles status a1b2 done

# Update title or description
pebbles update a1b2 --title "New title"
pebbles update a1b2 --description "New description"
```

### Link Issues

```bash
# Make issue part of an epic
pebbles link <issue-id> part_of <epic-id>

# Mark dependencies
pebbles link <issue-id> required_for <other-id>

# Define ordering
pebbles link <issue-id> scheduled_before <milestone-id>

# Remove relationship
pebbles unlink <issue-id> part_of <epic-id>
```

**Relationship types:** `part_of`, `required_for`, `scheduled_before`

### Delete Issues

```bash
pebbles delete <id>
```

## Usage Examples

### Planning a Feature

```bash
# Create an epic
pebbles new "User Authentication" --issue-type epic
# Created epic a1b

# Break it down
pebbles new "Login form design" --issue-type ux
# Created ux_problem e5f

pebbles new "JWT implementation" --issue-type design
# Created design i9j

# Link to epic
pebbles link e5f part_of a1b
pebbles link i9j part_of a1b

# Add dependencies
pebbles link e5f required_for i9j  # Design before implementation
```

### Daily Workflow

```bash
# See what's in progress
pebbles list --status in_progress

# Start working on something
pebbles status a1b2 in_progress

# Complete it
pebbles status a1b2 done

# View what's left
pebbles list --status todo
```

### Bug Triage

```bash
# Create bugs
pebbles new "Null pointer in user service" --issue-type bug
pebbles new "Dashboard shows wrong date" --issue-type bug

# Prioritize
pebbles status a1b2 in_progress  # Critical bug
pebbles status c3d4 todo         # Lower priority
```

### Working with Relationships

```bash
# View an issue with relationships
pebbles show a1b2

# Example output shows:
# Relationships:
#   part of e5f6g7h8
#   required for i9j0k1l2
```

### Using Short IDs

```bash
# Full UUID not required - use shortest unambiguous prefix (min 3 chars)
pebbles show 53ed9aa8        # 8 chars always works
pebbles status 53ed done     # Shorter if unambiguous
pebbles update 53e --title "New title"  # Minimum 3 characters
```

### Shell Integration

```bash
# Count issues by status
pebbles list --status todo | wc -l

# Search issue files directly
grep -r "authentication" .pebbles/issues/

# List issues by type
ls .pebbles/issues/bug-*    # All bugs
ls .pebbles/issues/epic-*   # All epics
```

## Data Storage

Issues are stored in `.pebbles/`:

```
.pebbles/
├── index.json                                      # Issue metadata
└── issues/
    ├── bug-a1b2c3d4-login-button-broken.json      # Bugs
    ├── ux-e5f6g7h8-dashboard-redesign.json        # UX issues
    ├── epic-i9j0k1l2-user-authentication.json     # Epics
    └── uc-m3n4o5p6-user-can-login.json            # Usecases
```

Files are named `type-shortid-slug.json` for human-readable diffs and easy browsing.

## Version Control Integration

Every pebbles operation creates a jj change with a descriptive message:

```bash
jj log
# Shows:
# @ Update issue: Login button fixed
# ○ Create issue: Dashboard redesign
# ○ Link issues: Design required for implementation
```

### Syncing Across Machines

```bash
# Machine A
pebbles new "New task"
jj git push

# Machine B
jj git fetch
jj new main

pebbles list  # See the new task
```

## Troubleshooting

### "Pebbles not initialized"

```bash
pebbles init
```

### "No issue found with ID"

```bash
pebbles list  # See all valid IDs
```

### "Ambiguous ID"

Use more characters:

```bash
pebbles show a1b2  # Instead of just 'a'
```

### "Epic cannot be part of an epic"

Only bugs, ux_problems, usecases, designs, and validations can be part of epics. Epics and milestones cannot.

## Further Reading

- **[ARCHITECTURE.md](ARCHITECTURE.md)** - System design and architecture
- **[SPEC.md](SPEC.md)** - Data model and file format specification
- **[CONTRIBUTING.md](CONTRIBUTING.md)** - Development guide and how to contribute

## License

MIT
