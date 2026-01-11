# AI Agents Guide for Pebbles

This guide is for AI coding assistants (Claude, GPT, Copilot, etc.) working on the pebbles codebase.

## Essential Reading

**All AI agents must read and follow [CONTRIBUTING.md](CONTRIBUTING.md).**

This includes:
- Development workflow (build, test, install)
- Jujutsu version control workflow
- Code style and quality standards
- Documentation rules (evergreen content only)
- Testing requirements

## Core Documentation

Before making changes, understand the project:

1. **[README.md](README.md)** - What pebbles does and how users interact with it
2. **[ARCHITECTURE.md](ARCHITECTURE.md)** - Design philosophy and system architecture
3. **[SPEC.md](SPEC.md)** - Data model, file formats, and behavior specifications
4. **[CONTRIBUTING.md](CONTRIBUTING.md)** - Development guidelines (mandatory)

## Key Project Characteristics

**Simplicity:**
- JSON files, not databases
- Plain text tracked by version control
- No complex state machines
- Trust the code as documentation

**Version Control First:**
- Built for Jujutsu (jj)
- Every operation auto-commits
- Issues live in `.pebbles/` directory
- Filenames are meaningful: `type-shortid-slug.json`

**Development Approach:**
- Test with debug builds: `./target/debug/pebbles`
- Use installed version for real work: `pebbles`
- Install when done: `cargo install --path .`

## Workflow Summary

**For detailed workflow, see [CONTRIBUTING.md](CONTRIBUTING.md).**

Quick reference:
```bash
# Build for testing
cargo build
./target/debug/pebbles list

# Run quality checks
cargo test
cargo clippy
cargo fmt

# Install when feature complete
cargo install --path .
```

## Documentation Standards

**Critical: Follow [CONTRIBUTING.md Documentation Rules](CONTRIBUTING.md#documentation-rules)**

- ✅ Describe current behavior
- ✅ Link to code, don't duplicate it
- ✅ Evergreen content only
- ❌ No version numbers or dates
- ❌ No status markers or checkboxes
- ❌ No "planned features" or roadmaps

When code changes, update docs to match new behavior. Remove outdated statements. Don't add historical notes.

## Code Organization

See [ARCHITECTURE.md](ARCHITECTURE.md) for detailed architecture.

Key modules:
- `src/models.rs` - Data structures and validation logic
- `src/operations.rs` - Business logic (CRUD operations)
- `src/storage.rs` - File I/O (read/write JSON files)
- `src/jj.rs` - Jujutsu integration (auto-commit)
- `src/display.rs` - Terminal output formatting
- `src/cli.rs` - Command-line interface definitions
- `src/main.rs` - Entry point and command routing

## Extension Points

Common tasks:
- **New commands** - Edit `cli.rs`, `operations.rs`, `main.rs`
- **New issue types** - Edit `models.rs`, `display.rs`
- **New relationships** - Edit `models.rs` RelationType enum
- **Display changes** - Edit `display.rs`

See [ARCHITECTURE.md Extension Points](ARCHITECTURE.md#extension-points) for implementation details.

## Pebbles Self-Tracking

Pebbles manages its own development with pebbles issues in `.pebbles/issues/`.

When working on features:
```bash
# Create issue
pebbles new "Feature description" --issue-type usecase

# Link to epic
pebbles link <issue-id> part_of <epic-id>

# Track progress
pebbles status <id> in_progress
pebbles status <id> done
```

## Quality Standards

From [CONTRIBUTING.md](CONTRIBUTING.md):
- Run `cargo fmt` before committing
- Fix `cargo clippy` warnings
- Add tests for new functionality
- Update documentation when behavior changes
- Write clear commit messages with `jj describe`

## Questions?

1. Check [CONTRIBUTING.md](CONTRIBUTING.md) first
2. Review [ARCHITECTURE.md](ARCHITECTURE.md) for design context
3. Look at [SPEC.md](SPEC.md) for specifications
4. Read the code - it's the source of truth
5. Ask the user if still unclear
