# Pebbles Architecture

This document explains the design philosophy, system architecture, and key technical decisions behind Pebbles.

## Design Philosophy

Pebbles is built on three core principles:

### 1. Simplicity

No complex database schemas, no fragile state machines. Issues are plain JSON files. The index is a simple list. No magic, no surprises.

**Benefits:**
- Easy to understand and debug
- No database corruption
- No migration scripts
- Works with standard Unix tools (grep, sed, awk)

### 2. Reliability

Plain text files tracked by version control eliminate entire categories of failure:
- No database corruption
- No lost data (everything is in version control)
- No complex recovery procedures
- No backup strategies needed (version control is the backup)

### 3. Version Control First

Let jj do what it does best: track changes, merge conflicts, sync across machines. Pebbles doesn't reimplement version control—it embraces it.

**Why Jujutsu?**
- Every change is automatically tracked (no git add/commit dance)
- Conflicts are first-class and can be resolved incrementally
- History rewriting is safe and intuitive
- Works with git remotes for compatibility

## System Architecture

### High-Level Overview

```
┌─────────────┐
│     CLI     │  Command-line interface (clap)
└──────┬──────┘
       │
┌──────▼──────┐
│ Operations  │  Business logic and validation
└──┬────────┬─┘
   │        │
   ▼        ▼
┌─────┐  ┌─────┐
│Store│  │  JJ │  Storage and version control
└─────┘  └─────┘
   │        │
   ▼        ▼
┌─────────────┐
│ .pebbles/   │  File system
│   + .jj/    │
└─────────────┘
```

### Module Responsibilities

**[`src/main.rs`](src/main.rs)** - Entry point
- Parse CLI arguments
- Route commands to operations
- Handle errors and display results

**[`src/cli.rs`](src/cli.rs)** - Command-line interface
- Define command structure with clap
- Parse arguments and options
- Generate help text

**[`src/models.rs`](src/models.rs)** - Data structures
- Define `Issue`, `Status`, `IssueType`, `RelationType`
- Implement serialization/deserialization
- Business logic: validation, filename generation, relationship management

**[`src/storage.rs`](src/storage.rs)** - File I/O
- Load/save index and issues
- Handle file renames when titles change
- Abstract file system operations

**[`src/operations.rs`](src/operations.rs)** - Business logic
- Coordinate between storage and jj
- Implement CRUD operations
- Validate operations (e.g., relationship rules)
- Resolve short IDs to UUIDs

**[`src/jj.rs`](src/jj.rs)** - Jujutsu integration
- Initialize repositories
- Create changes with descriptive messages
- Execute jj commands

**[`src/display.rs`](src/display.rs)** - Terminal output
- Format issue lists and details
- Colorize output by type and status
- Pretty-print with Unicode borders

### Data Flow Examples

**Creating an issue:**
1. CLI parses `new` command with title, description, type
2. Operations creates `Issue` struct with UUID
3. Operations saves issue via Storage (generates filename)
4. Storage writes JSON file to `.pebbles/issues/`
5. Storage updates index with ID → filename mapping
6. JJ creates change: "Create {type}: {title}"
7. Display shows created issue to user

**Updating a title:**
1. CLI parses `update` command with ID and new title
2. Operations resolves short ID to UUID
3. Operations loads issue via Storage
4. Operations updates title and `updated_at` timestamp
5. Storage generates new filename (title changed)
6. Storage deletes old file, writes new file
7. Storage updates index with new filename
8. JJ creates change: "Update {type}: {title}"
9. Display shows updated issue to user

See [SPEC.md](SPEC.md) for complete behavioral specifications.

## Key Design Decisions

### Individual JSON Files

**Decision:** Store each issue as a separate JSON file instead of one large file or database.

**Rationale:**
- **Version control friendly:** Each issue change only affects one file
- **No corruption cascade:** One corrupted file doesn't break everything
- **Merge friendly:** Multiple people can edit different issues simultaneously
- **Tool friendly:** Standard Unix tools can process individual files

**Trade-offs:** Not ideal for millions of issues (but that's not our use case).

### Meaningful Filenames

**Decision:** Name files `{type}-{shortid}-{slug}.json` instead of `{uuid}.json`.

**Rationale:**
- **Human-readable diffs:** See what changed in `jj log` without opening files
- **Easy browsing:** Know what an issue is about from filename
- **Natural sorting:** Issues group by type
- **Grep friendly:** Can search by filename patterns

**Trade-offs:** Must handle renames when titles change (appears as delete + add in jj).

See [`Issue::file_name()`](src/models.rs#L178-L186) for implementation.

### UUIDs Instead of Sequential IDs

**Decision:** Use UUID v4 for issue IDs instead of sequential integers.

**Rationale:**
- **No coordination needed:** Multiple people can create issues offline simultaneously
- **No merge conflicts:** IDs never collide across branches or machines
- **Globally unique:** Issues can be referenced across repositories

**Trade-offs:** UUIDs are longer, but short ID prefixes solve this for human use.

### No Database

**Decision:** JSON files instead of SQLite, PostgreSQL, etc.

**Rationale:**
- **Simpler:** No schema migrations, no connection management
- **More reliable:** Databases can corrupt (especially SQLite in version control)
- **More inspectable:** Any text editor can view/edit issues
- **Better for version control:** Databases don't diff/merge well

**Trade-offs:** No SQL queries, but filtering in-memory works fine for < 10,000 issues.

### Jujutsu Over Git

**Decision:** Built for jj, not git (though jj can coexist with git).

**Rationale:**
- **Automatic commits:** No need to remember `git add` and `git commit`
- **Better conflicts:** Conflicts are first-class and can be resolved incrementally
- **Flexible history:** Easy to reorganize and describe changes after the fact
- **Perfect for automation:** No git staging area confusion

**Compatibility:** jj works with git remotes via `jj git init --colocate`.

### Three Relationship Types

**Decision:** Explicit `part_of`, `required_for`, `scheduled_before` instead of generic "links".

**Rationale:**
- **Clear semantics:** Each type has specific meaning
- **Type-specific validation:** Can enforce rules like "epics can't be part of epics"
- **Better queries:** Can find all epic members or all dependencies separately

See [`RelationType`](src/models.rs#L101-L107) and validation in [`Issue::add_relationship()`](src/models.rs#L188-L207).

### HashMap Metadata

**Decision:** Free-form `HashMap<String, String>` for custom fields instead of predefined fields.

**Rationale:**
- **Extensible without code changes:** Users can add custom fields without modifying Pebbles
- **Type-specific fields:** Different issue types can have different metadata
- **Future-proof:** New features can add metadata without schema migrations

**Trade-offs:** No type safety for metadata values. CLI commands for metadata management not yet implemented.

## Performance Characteristics

Current implementation is optimized for:
- **Repository size:** < 10,000 issues
- **Team size:** Single user or small teams
- **Access pattern:** Local file system

**Current performance:**
- **Issue loading:** O(n) where n = number of issues
- **ID resolution:** O(n) linear search through index
- **File operations:** O(1) per issue

**Optimization opportunities (if needed):**
- Use `HashMap<Uuid, String>` for O(1) index lookups
- Add in-memory cache for frequently accessed issues
- Parallelize issue loading with rayon

For typical usage (< 1000 issues), current performance is negligible (< 100ms for most operations).

## Extension Points

### Adding New Commands

1. Add variant to [`Commands` enum](src/cli.rs#L11-L93)
2. Add method to [`Operations` struct](src/operations.rs)
3. Wire up in [`main.rs`](src/main.rs)
4. Update display in [`display.rs`](src/display.rs) if needed

### Adding New Issue Types

1. Add variant to [`IssueType` enum](src/models.rs#L40-L50)
2. Add `FromStr` case for parsing
3. Add short code in [`short_code()`](src/models.rs#L88-L98)
4. Add color in [`display.rs`](src/display.rs)
5. Update validation rules if needed

### Adding New Relationship Types

1. Add variant to [`RelationType` enum](src/models.rs#L101-L107)
2. Add `FromStr` case for parsing
3. Add validation in [`Issue::add_relationship()`](src/models.rs#L188-L207)
4. Add display formatting in [`display.rs`](src/display.rs)

### Using Custom Metadata

Metadata is available in the data model but not yet exposed via CLI. Options:

1. **Direct JSON manipulation:** Edit issue files directly
2. **Future CLI commands:** `pebbles metadata set <id> <key> <value>`
3. **Scripts:** Use `jq` or similar to manipulate metadata

### Testing Functional Core vs Imperative Shell

**Question:** Does logic need to be extracted from methods?

**Answer:** No - logic functions are allowed to be methods. The key testability criterion is: *can this be tested in isolation without calling effectful functions?*

**Example of testable method:**
```rust
pub fn add_relationship(&mut self, relation_type: RelationType, target_id: Uuid) -> Result<()>
```
This is testable because `Issue` can be constructed directly with `Issue::new` (no I/O):
```rust
let mut issue = Issue::new("Test".to_string(), None, IssueType::Bug);
let result = issue.add_relationship(RelationType::PartOf, epic_id);
assert!(result.is_err());  // Cannot add Bug to Epic
```

**Example of method needing extraction:**
```rust
// NOT testable as-is because requires loading index
fn resolve_id(&self, id_str: &str) -> Result<Uuid> {
    let index = self.storage.load_index()?;  // I/O dependency
    // ... pure matching logic
}

// Extract pure logic for testability
pub fn match_id_prefix(id_str: &str, known_ids: &[Uuid]) -> Result<Uuid>
```
Now testable without storage.

## Security Considerations

### File System

- Issues stored as plain text JSON (no encryption by default)
- Standard file system permissions apply
- Sensitive data should not be stored in issues without additional encryption

### Input Validation

- All user input is validated before processing
- UUIDs validated before file operations
- Relationship types validated against rules
- No shell command injection (jj arguments passed safely)

### Version Control

- All issue changes tracked by jj
- Deleted issues can be recovered from history
- Accidentally committed sensitive data can be removed with `jj abandon` or `jj obslog` operations

## Testing Strategy

Pebbles follows an "imperative shell, functional core" design for testing:

**Imperative Shell** (CLI → Operations → Storage/JJ):
- Tested via end-to-end trycmd integration tests
- Validates actual behavior with real file I/O
- Tests shell orchestration (loading, saving, committing)
- Example: Tests workflows in `tests/workflows/*.trycmd`

**Functional Core** (pure business logic):
- Effects are explicit in function signatures (moves, mutable borrows)
- Tested via unit tests with simple inputs/outputs
- Validates algorithms independent of I/O
- Examples: ID matching, dependency checking, statistics

### Testing Layers

```
┌─────────────────────────────────┐
│   Layer 3: CLI Tests            │  trycmd - end-to-end workflows
│   covers: shell orchestration   │
└─────────────────────────────────┘
         ↓
┌─────────────────────────────────┐
│   Layer 2: Logic Tests          │  unit tests for pure functions
│   covers: algorithms,           │
│            validation,          │
│            aggregation          │
└─────────────────────────────────┘
         ↓
┌─────────────────────────────────┐
│   Layer 1: Model Tests          │  unit tests for Issue methods
│   covers: business rules,       │
│            invariants,          │
│            constraints          │
└─────────────────────────────────┘
```

### Test Examples

**Model tests (src/models.rs):**
- `Issue::add_relationship` - validation rules, duplicate prevention
- `Issue::file_name` - slug generation, filename formatting
- `Index` - ID to filename mapping

**Logic tests (src/operations.rs):**
- ID matching - exact UUIDs, prefix resolution, ambiguity detection
- Dependency checking - blocked status, unblocked issues
- Statistics aggregation - counts by status/type, epic progress

**CLI tests (tests/workflows/*.trycmd):**
- Core workflows: create, update, link, delete
- Edge cases: validation errors, missing data
- Integration: multiple operations together

### Test Mode

For integration testing without affecting real data:
- Set `PEBBLES_DB_PATH` to custom directory
- Create `.no_jj` marker file to skip jj operations
- Enables safe testing of editing commands

See [CONTRIBUTING.md](CONTRIBUTING.md) and [TESTABILITY_ANALYSIS.md](TESTABILITY_ANALYSIS.md) for detailed testing guidance.

## Comparison with Alternatives

### vs GitHub Issues
- **Pebbles:** Offline-first, local, version-controlled, no server required
- **GitHub:** Online, centralized, web-based, requires internet

### vs Jira
- **Pebbles:** Simple, fast, hackable, developer-focused
- **GitHub:** Complex, slow, enterprise-focused, point-and-click UI

### vs TODO.txt
- **Pebbles:** Structured data, rich relationships, metadata, types
- **TODO.txt:** Plain text, simpler, less structure

### vs Git Issues (legacy system)
- **Pebbles:** Jujutsu automatic tracking, simpler workflow
- **Git Issues:** Manual git add/commit, more overhead

## Future Architecture Considerations

### Scalability

If repositories grow beyond 10,000 issues:
- Add caching layer for frequently accessed issues
- Use HashMap-based index for O(1) lookups
- Implement incremental loading (load issues on demand)
- Consider splitting large epics into separate sub-repositories

### Concurrent Access

Current design assumes single-user operation. For concurrent use:
- Jujutsu handles file conflicts automatically
- Index conflicts require manual resolution
- Could implement optimistic locking for index updates
- Could use file-based locking for write operations

### Search

For better search capabilities:
- Add full-text search index (tantivy or similar)
- Implement fuzzy matching for titles (fuzzy-matcher already included)
- Add semantic search with embeddings
- Cache search results

### Web UI

If a web interface is added:
- Pebbles core remains CLI
- Web UI reads `.pebbles/` directly (read-only) or calls CLI (read-write)
- WebSocket for live updates when files change
- Static site generation for browsing issues offline
