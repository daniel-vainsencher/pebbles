# Pebbles Quick Start Guide

## Installation

```bash
cd pebbles
cargo build --release
cargo install --path .
```

## Basic Usage

### Initialize a Repository

```bash
cd your-project
pebbles init
```

### Create Issues

```bash
# Simple bug (default type)
pebbles new "Fix login issue"

# With description
pebbles new "Redesign dashboard" -d "Current UI is confusing for new users"

# With specific type
pebbles new "User can export data" --issue-type usecase
pebbles new "Authentication architecture" --issue-type design
pebbles new "Profile page redesign" --issue-type ux
pebbles new "Q1 Release" --issue-type milestone
pebbles new "Authentication Feature" --issue-type epic
```

### List Issues

```bash
# All issues
pebbles list

# By status
pebbles list --status todo
pebbles list --status in_progress
pebbles list --status blocked
pebbles list --status done

# By type
pebbles list --issue-type bug
pebbles list --issue-type epic

# Combined
pebbles list --status in_progress --issue-type bug
```

### View Issue Details

```bash
pebbles show <id>
pebbles show a1b2    # Short ID works!
```

### Update Issues

```bash
# Change status
pebbles status <id> in_progress
pebbles status <id> blocked
pebbles status <id> done

# Update title
pebbles update <id> --title "New title"

# Update description
pebbles update <id> --description "New description"

# Update both
pebbles update <id> --title "Better title" --description "More details"
```

### Link Issues (Relationships)

```bash
# Make issue part of an epic
pebbles link <issue-id> part_of <epic-id>

# Mark a dependency
pebbles link <issue-id> required_for <other-id>

# Define ordering
pebbles link <issue-id> scheduled_before <other-id>

# Remove a relationship
pebbles unlink <issue-id> part_of <epic-id>
```

### Delete Issues

```bash
pebbles delete <id>
```

## Issue Types

- **bug** - Software defects and errors
- **ux_problem** (or ux) - User experience issues
- **usecase** (or use-case) - User stories and requirements
- **design** - Software design and architecture problems
- **validation** - Validation features and checks
- **epic** - Container for grouping related issues
- **milestone** - For prioritization and ordering

## Relationship Types

- **part_of** - Issue is part of an epic (only certain types allowed)
- **required_for** - This issue must be done before another
- **scheduled_before** - Ordering constraint

## Status Values

- **todo** - Not started
- **in_progress** - Currently being worked on
- **blocked** - Blocked by external factors
- **done** - Completed

## Pro Tips

### Use Short IDs

You only need the first few characters of the UUID:

```bash
pebbles show a1b2c3d4    # Instead of full UUID
pebbles status a1b2 done
```

### View Files Directly

Issues are just JSON files:

```bash
ls .beads/issues/
cat .beads/issues/bug-a1b2c3d4-login-broken.json
```

### Check jj History

See what changes were made:

```bash
jj log
jj show
```

### Grep for Issues

```bash
grep -r "authentication" .beads/issues/
ls .beads/issues/bug-*     # All bugs
ls .beads/issues/epic-*    # All epics
```

## Example Workflow

### Planning a Feature

```bash
# Create an epic
pebbles new "User Profile Feature" --issue-type epic
# Created: epic-a1b2c3d4

# Break it down
pebbles new "Design profile data model" --issue-type design
# Created: des-e5f6g7h8

pebbles new "Profile page UI" --issue-type ux
# Created: ux-i9j0k1l2

pebbles new "User can view profile" --issue-type usecase
# Created: uc-m3n4o5p6

# Link them
pebbles link e5f6 part_of a1b2
pebbles link i9j0 part_of a1b2
pebbles link m3n4 part_of a1b2

# Add dependency
pebbles link i9j0 required_for m3n4  # UI needed before usecase
```

### Working Through Tasks

```bash
# See what's todo
pebbles list --status todo

# Start working
pebbles status e5f6 in_progress

# Complete it
pebbles status e5f6 done

# If blocked
pebbles status i9j0 blocked
pebbles update i9j0 --description "Blocked: waiting for design assets"
```

### Review Progress

```bash
# What's done?
pebbles list --status done

# What's in progress?
pebbles list --status in_progress

# What's blocking us?
pebbles list --status blocked

# Show epic with all its parts
pebbles show a1b2    # Shows relationships
```

## Common Patterns

### Bug Triage

```bash
pebbles new "Bug: Can't save user settings" --issue-type bug
pebbles new "Bug: Slow page load" --issue-type bug
pebbles new "Bug: Broken link in footer" --issue-type bug

# Prioritize by updating status
pebbles status <critical-bug> in_progress
pebbles status <minor-bug> todo
```

### Sprint Planning

```bash
pebbles new "Sprint 5" --issue-type milestone
# Link all sprint tasks
pebbles link <task1> scheduled_before <milestone>
pebbles link <task2> scheduled_before <milestone>
```

### Refactoring Tracking

```bash
pebbles new "Refactoring Epic" --issue-type epic
pebbles new "Extract payment module" --issue-type design
pebbles new "Remove deprecated APIs" --issue-type design
pebbles link <task1> part_of <epic>
pebbles link <task2> part_of <epic>
```

## Troubleshooting

### "Pebbles not initialized"

```bash
pebbles init
```

### "No issue found with ID"

Make sure you're using the right ID:

```bash
pebbles list    # See all IDs
```

### "Ambiguous ID"

Use more characters:

```bash
pebbles show a1    # Ambiguous
pebbles show a1b2  # Unique
```

### "Epic cannot be part of an epic"

Validation rule - only bugs, ux_problems, usecases, designs, and validations can be part of epics:

```bash
# This fails:
pebbles link <epic1> part_of <epic2>

# This works:
pebbles link <bug-id> part_of <epic-id>
```

## File Structure

```
your-project/
├── .jj/                  # Jujutsu repository
└── .beads/
    ├── index.json        # Issue metadata
    └── issues/
        ├── bug-a1b2c3d4-login-broken.json
        ├── ux-e5f6g7h8-dashboard-redesign.json
        ├── epic-i9j0k1l2-auth-feature.json
        └── uc-m3n4o5p6-user-can-login.json
```

## Next Steps

- Read [README.md](README.md) for full documentation
- Check [DESIGN.md](DESIGN.md) for architecture details
- See [EXAMPLES.md](EXAMPLES.md) for more examples
- View [PROGRESS.md](PROGRESS.md) for current status and roadmap
