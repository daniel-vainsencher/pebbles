# Contributing to Pebbles

Thank you for your interest in contributing to Pebbles! This guide will help you get started.

## Development Setup

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) 1.70 or later
- [Jujutsu (jj)](https://github.com/martinvonz/jj) installed and in PATH
- Familiarity with Rust and command-line tools

### Building

```bash
# Clone the repository
git clone <repository-url>
cd pebbles

# Build in debug mode (fast builds for testing)
cargo build

# Build in release mode (optimized for performance)
cargo build --release

# Run without installing
cargo run -- <command>
```

### Development Workflow

**When developing new features:**

1. **Test with debug builds** - Fast compilation for rapid iteration:
   ```bash
   cargo build
   ./target/debug/pebbles <command>
   ```

2. **Use installed version for regular work** - Use the `pebbles` command for normal usage:
   ```bash
   pebbles list
   pebbles new "My issue"
   ```

3. **Install when feature is complete** - Replace the installed version:
   ```bash
   cargo install --path .
   # Now `pebbles` uses the new version
   ```

This keeps your development and production environments separate. Debug builds are much faster to compile (~1s vs ~30s for release), making the test-fix-test cycle faster.

### Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_name
```

### Quality Assurance Commands

```bash
# Check code without building
cargo check

# Run clippy for linting
cargo clippy --all-targets --all-features

# Format code
cargo fmt

# Check formatting without modifying
cargo fmt -- --check

# Build documentation
cargo doc --open
```

## Jujutsu Workflow

Pebbles development uses jj for version control. Here's the standard workflow:

### Making Changes

```bash
# Check current status
jj status

# Make your code changes
# (jj automatically tracks changes in the working copy)

# Describe your changes
jj describe -m "Brief description of changes

More detailed explanation if needed."

# View your changes
jj show

# See history
jj log
```

### Committing Changes

Unlike git, jj doesn't have a staging area. Changes are automatically tracked in your working copy change.

**To commit only specific files:**
```bash
jj commit -m "Description" file1.rs file2.rs
```

**To commit all changes:**
```bash
jj commit -m "Description"
```

This creates a new change and moves your working copy to a new empty change.

### Fixing Mistakes

```bash
# Undo last operation
jj undo

# Restore files from parent
jj restore <files>

# Abandon current change (discards it)
jj abandon

# Edit change description
jj describe
```

### Working with Multiple Changes

```bash
# Create a new change
jj new

# Edit an existing change
jj edit <change-id>

# Squash current change into parent
jj squash

# Move changes to a different parent
jj rebase -d <destination>
```

### Syncing with Remotes

```bash
# Fetch from remote (if using git colocated repo)
jj git fetch

# Push to remote
jj git push

# Create branch
jj bookmark create <name>

# Push branch
jj git push --bookmark <name>
```

### Useful jj Commands

```bash
# Show recent history with graph
jj log -r 'all()'

# Show changes in a specific change
jj show <change-id>

# View operation log (history of jj commands)
jj op log

# View evolution of a change
jj evolog

# Check for conflicts
jj status
```

For more details, see `jj help` or the [Jujutsu documentation](https://jj-vcs.github.io/jj/).

## Code Structure

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed architecture documentation.

**Module overview:**
- [`src/main.rs`](src/main.rs) - Entry point and command routing
- [`src/cli.rs`](src/cli.rs) - Command-line interface definitions
- [`src/models.rs`](src/models.rs) - Data structures and validation
- [`src/storage.rs`](src/storage.rs) - File I/O operations
- [`src/operations.rs`](src/operations.rs) - Business logic
- [`src/jj.rs`](src/jj.rs) - Jujutsu integration
- [`src/display.rs`](src/display.rs) - Terminal output formatting

## Development Guidelines

### Code Style

- Follow Rust standard style (`cargo fmt`)
- Use `clippy` and fix warnings (`cargo clippy`)
- Write clear, descriptive variable names
- Add comments for non-obvious logic
- Keep functions focused and small

### Error Handling

- Use `anyhow::Result` for error propagation
- Add context to errors with `.context()` or `.with_context()`
- Return user-friendly error messages
- Don't panic in library code

### Testing

- Add tests for new functionality
- Test edge cases and error conditions
- Use descriptive test names
- Keep tests focused on one behavior

### Adding New Features

Before implementing a feature:
1. Create a pebbles issue describing the feature
2. Discuss the design approach
3. Update relevant documentation
4. Write tests
5. Implement the feature
6. Update documentation again if behavior changed

## Documentation Rules

Pebbles documentation follows strict guidelines to ensure it remains useful and accurate:

### 1. Evergreen Content Only

**Do not include:**
- Version numbers or dates in content ("`v0.2.0`", "as of January 2026")
- Status markers ("`✅ Complete`", "`🔄 In Progress`", "`[ ]` TODO checkboxes")
- Historical comparisons ("`v0.1.0 → v0.2.0`", "we used to do X")
- Roadmaps or future plans ("planned features", "coming soon")
- Changelogs or release notes

**Do include:**
- Current capabilities and behavior
- How things work now
- Design rationale (why, not when)

**Example - Bad:**
```markdown
## Status
✅ v0.2.0 Complete
- [x] Issue types (added in v0.2.0)
- [ ] TUI (planned for v0.3.0)
```

**Example - Good:**
```markdown
## Features
- Issue types (bug, ux, usecase, etc.)
- See [GitHub issues](link) for feature requests
```

### 2. No Repetition

**Avoid duplicating:**
- Code in documentation (link to code instead)
- Documentation in multiple places (choose one canonical location)
- Information already clear from reading the code

**Use markdown links to code:**
```markdown
See [`Issue` struct](src/models.rs#L139-L151) for the complete definition.
```

**Example - Bad:**
```markdown
The Issue struct has these fields:
- id: Uuid
- title: String
- description: Option<String>
- status: Status
...
```

**Example - Good:**
```markdown
The complete data model is defined in [`src/models.rs`](src/models.rs).
Key types: `Issue`, `Status`, `IssueType`, `RelationType`.
```

### 3. Link to Code, Don't Duplicate It

When documenting behavior:
- Explain at a high level
- Link to implementation for details
- Trust readers to read code if they need specifics

**Example - Bad:**
```markdown
## File Naming

Files are named using this algorithm:
```rust
pub fn file_name(&self) -> String {
    let slug = slug::slugify(&self.title);
    let short_slug = if slug.len() > 40 {
        &slug[..40]
    } else {
        &slug
    };
    format!("{}-{}-{}.json", self.issue_type.short_code(), self.short_id(), short_slug)
}
```

**Example - Good:**
```markdown
## File Naming

Files are named `{type}-{shortid}-{slug}.json` where type is the issue type short code, shortid is the first 8 characters of the UUID, and slug is a URL-safe version of the title (max 40 chars).

Implementation: [`Issue::file_name()`](src/models.rs#L178-L186)
```

### 4. Documentation Structure

- **README.md** - What is it, why use it, how to get started, basic usage
- **ARCHITECTURE.md** - Design philosophy, system architecture, key decisions
- **SPEC.md** - Data model, file formats, behavior specifications
- **CONTRIBUTING.md** - Development guide, workflow, documentation rules (this file)

### 5. Updating Documentation

When changing code:
- Update affected documentation to match new behavior
- Remove any statements that are no longer true
- Don't add historical notes about what changed (that's what version control is for)
- Keep documentation focused on current state

### 6. Checking Documentation

Before committing documentation:
- [ ] No version numbers or dates in content
- [ ] No status markers or checkboxes
- [ ] No "v0.x → v0.y" comparisons
- [ ] No roadmaps or planned features
- [ ] Linked to code instead of duplicating it
- [ ] No repetition between docs
- [ ] Describes current behavior accurately

## Pull Request Process

1. Create a pebbles issue describing the change
2. Make changes in a new jj change
3. Write or update tests
4. Update documentation if needed
5. Run QA commands (fmt, clippy, test)
6. Describe your change clearly with `jj describe`
7. Push to a branch with `jj git push --bookmark <branch-name>`
8. Create a pull request
9. Address review feedback
10. Wait for approval and merge

## Communication

- Use pebbles issues for bug reports and feature requests
- Use pull requests for code discussions
- Be respectful and constructive
- Ask questions if anything is unclear

## Testing Pebbles with Pebbles

We dogfood! Use pebbles to manage pebbles development:

```bash
# Track a bug
pebbles new "Fix ID resolution edge case" --issue-type bug

# Track a feature
pebbles new "Add metadata CLI commands" --issue-type usecase

# Track refactoring
pebbles new "Extract validation logic" --issue-type design

# Link to epic
pebbles link <issue-id> part_of <epic-id>

# Update status as you work
pebbles status <id> in_progress
pebbles status <id> done
```

## Questions?

If you have questions about contributing:
- Check existing documentation (README, ARCHITECTURE, SPEC)
- Look at existing code for examples
- Create an issue to discuss
- Ask in pull request comments

Thank you for contributing to Pebbles!
