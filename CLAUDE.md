# Claude Code Guide for Pebbles

This guide is for Claude Code (Anthropic's AI coding assistant) working on the pebbles codebase.

## Core Documentation

**Read these first:**
- **[CONTRIBUTING.md](CONTRIBUTING.md)** - Development workflow, build process, coding guidelines, documentation rules
- **[README.md](README.md)** - What pebbles is, how to use it
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - System design and technical decisions
- **[SPEC.md](SPEC.md)** - Data model and file format specifications

## Quick Reference

**Project structure:**
```
src/
├── main.rs         - Entry point and command routing
├── cli.rs          - Command-line interface (clap)
├── models.rs       - Data structures (Issue, Status, IssueType, etc.)
├── storage.rs      - File I/O for issues and index
├── operations.rs   - Business logic and validation
├── jj.rs           - Jujutsu version control integration
└── display.rs      - Terminal output formatting
```

**Key concepts:**
- Issues are JSON files in `.pebbles/issues/`
- Index maps UUIDs to filenames in `.pebbles/index.json`
- Every operation creates a jj change automatically
- Filenames are meaningful: `{type}-{shortid}-{slug}.json`

## Development Workflow

**Follow [CONTRIBUTING.md](CONTRIBUTING.md) for:**
- Build and test commands
- Debug vs release builds
- When to use `cargo install --path .`
- Jujutsu workflow
- Code style and testing

**Critical: Development vs Production**
- Test new features with `./target/debug/pebbles`
- Use regular `pebbles` command for normal work
- Install when done: `cargo install --path .`

## Documentation Rules

**From [CONTRIBUTING.md](CONTRIBUTING.md#documentation-rules):**
- Evergreen content only (no versions, dates, status markers)
- Link to code, don't duplicate it
- No repetition across docs
- Update when behavior changes

## Using Pebbles to Track Pebbles Work

When working on pebbles:

```bash
# Create issues for features/bugs
pebbles new "Your feature description" --issue-type usecase

# Link to epics
pebbles link <issue-id> part_of <epic-id>

# Update status as you work
pebbles status <id> in_progress
pebbles status <id> done
```

## Common Tasks

**Adding a new command:**
1. Add variant to `Commands` enum in [`src/cli.rs`](src/cli.rs)
2. Add method to `Operations` in [`src/operations.rs`](src/operations.rs)
3. Wire up in [`src/main.rs`](src/main.rs)
4. Update display in [`src/display.rs`](src/display.rs) if needed

**Adding a new issue type:**
1. Add variant to `IssueType` enum in [`src/models.rs`](src/models.rs)
2. Add `FromStr` case for parsing
3. Add short code in `short_code()` method
4. Add color in [`src/display.rs`](src/display.rs)
5. Update validation rules if needed

**See [ARCHITECTURE.md](ARCHITECTURE.md#extension-points) for more details.**

## Testing

```bash
# Build debug version for testing
cargo build
./target/debug/pebbles <command>

# Run tests
cargo test

# Check code quality
cargo clippy
cargo fmt --check
```

## Questions?

Refer back to:
- [CONTRIBUTING.md](CONTRIBUTING.md) - Development practices
- [ARCHITECTURE.md](ARCHITECTURE.md) - Design decisions
- [SPEC.md](SPEC.md) - Detailed specifications
- Code itself - Trust the implementation as source of truth
