# Pebbles Design Document

## Philosophy

Pebbles is built on three core principles:

1. **Simplicity** - No complex database schemas, no fragile state machines
2. **Reliability** - Rock-solid data storage with zero corruption risk
3. **Version Control First** - Let jj do what it does best

## Why Not Git?

While the original "beads" used git, Jujutsu offers significant advantages:

### Automatic Commits
Git requires explicit staging and committing. Jujutsu tracks changes automatically:
- Every edit creates a new change
- No need to remember to commit
- No lost work
- Perfect for automated tools and AI agents

### Better Conflict Resolution
Jujutsu treats conflicts as first-class:
- Conflicts are stored in the working copy
- Multiple people can edit the same issue
- Conflicts are resolved incrementally
- No "merge hell"

### Flexible History
Jujutsu makes history rewriting safe and easy:
- Describe changes after the fact
- Reorganize issue changes
- Clean up messy histories
- Safe undo/redo operations

### Git Compatibility
Jujutsu can coexist with git:
- `jj git init --colocate` creates both .jj and .git
- Push/pull to git remotes works seamlessly
- Existing git workflows are preserved

## Data Model

### Simple JSON Files

Each issue is stored as a standalone JSON file. Why?

**Pros:**
- Human readable
- Easy to debug
- No database migrations
- grep/sed/awk friendly
- Version control friendly
- No corruption risk

**Cons:**
- Not ideal for millions of issues (but that's not our use case)
- No relational queries (we don't need them)

### UUID-based IDs

Each issue gets a UUID v4. Why?

- No coordination needed for distributed creation
- Globally unique across repositories
- No ID conflicts when merging branches
- Can create issues offline

### Short ID Support

Full UUIDs are unwieldy (53ed9aa8-b717-4d38-9760-c14bd4dbe115), so we support prefixes:

```bash
pebbles show 53ed9aa8  # First 8 chars
pebbles show 53ed      # Even shorter
```

The resolver:
1. First tries exact UUID match
2. Then tries prefix match
3. Fails if ambiguous (multiple matches)

### Index File

`index.json` contains just the list of issue UUIDs:

```json
{
  "issues": [
    "uuid1",
    "uuid2",
    "uuid3"
  ]
}
```

Why not scan the directory?
- Faster for large numbers of issues
- Preserves issue order
- Atomic updates (one file write)

Why not embedded in issues?
- Each issue is independent
- No circular dependencies
- Easy to add/remove issues

## Storage Strategy

### File Layout

```
.pebbles/
├── index.json           # Index of all issues
└── issues/
    ├── <uuid1>.json
    ├── <uuid2>.json
    └── ...
```

Clean, simple, no surprises.

### No Database

No sqlite, no postgres, no mongodb. Why?

1. **Corruption risk** - Databases can corrupt (especially sqlite)
2. **Complexity** - Migrations, schemas, connection management
3. **Tooling** - Need special tools to inspect
4. **Version control** - Databases don't diff/merge well

Plain JSON files solve all of these.

### No JSONL

The original "beads" used JSONL (JSON Lines). Problems:

1. **Append-only pain** - Hard to update/delete
2. **Compaction needed** - File grows forever
3. **Corruption risk** - One bad line breaks everything
4. **Poor diffing** - Hard to see what changed

Individual JSON files are better in every way.

## Jujutsu Integration

### Change Descriptions

Every operation updates the jj change description:

- `pebbles new` → "Create issue: <title>"
- `pebbles status` → "Update issue <title>: todo → done"
- `pebbles delete` → "Delete issue: <title>"

This creates a meaningful history:

```
$ jj log
@  Update issue Fix bug: todo → done
◆  Create issue: Fix bug in parser
◆  Create issue: Implement authentication
```

### Working Copy Changes

We use `jj describe` instead of `jj commit` because:
- Updates the current working copy change
- No need to create new changes constantly
- More natural for interactive work

For automation, you might want to use `jj new` between operations.

### No Git Hooks

We don't use git hooks because:
1. Jujutsu doesn't need them
2. Hooks are fragile and environment-dependent
3. Direct CLI integration is simpler

## CLI Design

### Subcommand Structure

Following standard Unix conventions:

```
pebbles <command> [arguments] [options]
```

Commands are verbs:
- `init` - Initialize
- `new` - Create
- `list` - List
- `show` - Show details
- `status` - Update status
- `update` - Update details
- `delete` - Delete

### Short IDs

Typing full UUIDs is painful:

```bash
# Bad
pebbles show 53ed9aa8-b717-4d38-9760-c14bd4dbe115

# Good
pebbles show 53ed
```

The resolver handles ambiguity gracefully.

### Colored Output

Status colors:
- Yellow: TODO (needs attention)
- Blue: IN_PROGRESS (being worked on)
- Green: DONE (completed)

This provides instant visual feedback.

## Error Handling

### Rust's Result Type

All operations return `Result<T, Error>`:

```rust
pub fn create(&self, title: String) -> Result<Issue>
```

This forces explicit error handling at every level.

### Anyhow for Ergonomics

Using `anyhow` for error handling:
- Automatic error context
- Easy error chaining
- Nice error messages

Example:

```rust
fs::write(&path, content)
    .context("Failed to write issue file")?;
```

### Graceful Degradation

If an issue file is corrupted:
- Warn the user
- Continue with other issues
- Don't crash

## Future Considerations

### Performance

Current design is optimized for:
- < 10,000 issues per repository
- Single-user or small team use
- Local file system access

For larger scale:
- Could add caching layer
- Could use memory-mapped files
- Could implement incremental indexing

### Concurrent Modifications

Currently, pebbles assumes single-user operation. For concurrent use:
- Jujutsu handles file conflicts automatically
- Index conflicts need resolution
- Could implement optimistic locking

### Search

Current `list` is simple. For better search:
- Could add full-text search with tantivy
- Could add query language
- Could add saved searches/filters

### API

Currently CLI-only. For programmatic access:
- Could add Rust library crate
- Could add JSON output mode
- Could add RPC server

## Comparison with Other Tools

### vs GitHub Issues
- **Pebbles**: Offline-first, local, version-controlled
- **GitHub**: Online, centralized, web-based

### vs Jira
- **Pebbles**: Simple, fast, hackable
- **Jira**: Complex, slow, enterprise-focused

### vs Linear
- **Pebbles**: Terminal-based, developer-focused
- **Linear**: GUI-based, team-focused

### vs TODO.txt
- **Pebbles**: Structured data, richer metadata
- **TODO.txt**: Plain text, simpler

### vs Org-mode
- **Pebbles**: Standalone tool, language-agnostic
- **Org-mode**: Emacs-based, tightly integrated

## Testing Strategy

### Unit Tests
Test individual functions in isolation:
- Issue creation
- Status parsing
- ID resolution

### Integration Tests
Test full workflows:
- Init → Create → List → Update → Delete

### Manual Testing
Real-world usage:
- Managing pebbles development with pebbles
- Dogfooding catches UX issues

## Maintenance Philosophy

### No Feature Creep
Resist adding features that:
- Complicate the data model
- Add new dependencies
- Make errors more likely

### Stability Over Features
Better to have 10 features that work perfectly than 100 features with bugs.

### Documentation First
Every feature needs:
- README entry
- Usage example
- Design rationale

## Lessons from "beads"

The original "beads" had issues:

1. **Mixed storage** - sqlite + jsonl + git was fragile
2. **Corruption prone** - sqlite corruption caused data loss
3. **Migration pain** - Schema changes broke compatibility
4. **Git complexity** - Manual commit/push was error-prone

Pebbles fixes all of these:

1. **Single storage** - Just JSON files
2. **No corruption** - Individual files can't corrupt the whole system
3. **No migrations** - JSON is self-describing
4. **Jujutsu simplicity** - Automatic versioning, no manual commits

## Conclusion

Pebbles is intentionally simple. It does one thing well: track issues in a version-controlled repository.

By building on Jujutsu's solid foundation and keeping the data model trivial, we achieve rock-solid reliability that "just works" for both humans and AI agents.
