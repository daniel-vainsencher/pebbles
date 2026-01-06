# Pebbles Development Progress

## Current Status

**Version:** 0.2.0 (Enhanced)
**Status:** Core features implemented and tested ✅

## Completed Features

### ✅ Phase 1: Basic Issue Management (v0.1.0)
- [x] Rust project structure with Cargo
- [x] Jujutsu (jj) integration layer
- [x] Basic CRUD operations for issues
- [x] Simple CLI with clap
- [x] JSON-based storage in `.beads/` directory
- [x] Status tracking (todo, in_progress, done)
- [x] Short ID support
- [x] Colored terminal output
- [x] Documentation (README, DESIGN, EXAMPLES)

### ✅ Phase 2: Enhanced Data Model (v0.2.0)
- [x] Issue types: bug, ux_problem, usecase, design, validation, epic, milestone
- [x] Relationship types: part_of, required_for, scheduled_before
- [x] Type-based validation rules (e.g., epics can't be part of other epics)
- [x] Meaningful file naming: `type-shortid-slug.json`
- [x] Automatic file renaming when title changes
- [x] Metadata field for extensibility
- [x] Blocked status for issues
- [x] Enhanced display with type and relationship information
- [x] Link/unlink commands for managing relationships

## Architecture

### Data Model

```rust
pub enum IssueType {
    Bug,         // Software defects
    UxProblem,   // User experience issues
    Usecase,     // User stories/requirements
    Design,      // Software design problems
    Validation,  // Validation features
    Epic,        // Grouping container
    Milestone,   // Prioritization/ordering
}

pub enum RelationType {
    PartOf,          // Issue is part of epic
    RequiredFor,     // Issue required for another
    ScheduledBefore, // Ordering constraint
}

pub struct Issue {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub issue_type: IssueType,
    pub tags: Vec<String>,
    pub relationships: Vec<Relationship>,
    pub metadata: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### File Structure

```
.beads/
├── index.json                              # Metadata: id → filename mapping
└── issues/
    ├── bug-7355bfc7-login-button-broken.json
    ├── ux-a3afb471-dashboard-redesign.json
    ├── epic-661d1abe-user-authentication-epic.json
    └── uc-417dce65-user-can-login-with-email.json
```

Benefits:
- **Human-readable**: File names tell you what the issue is about
- **Sortable**: Grouped by type, then by short ID
- **jj-friendly**: Diffs show meaningful file names
- **Searchable**: Can grep by filename pattern

### Validation Rules

- **Epic relationships**: Only bug, ux_problem, usecase, design, and validation can be `part_of` an epic
- **Self-consistency**: Epics and milestones cannot be part of other epics
- **Duplicate prevention**: Can't add the same relationship twice

## Usage Examples

### Creating Issues with Types

```bash
# Create a bug
pebbles new "Login button broken" -d "Button doesn't respond" --issue-type bug

# Create a UX problem
pebbles new "Dashboard is confusing" --issue-type ux

# Create an epic to group related work
pebbles new "User Authentication Epic" --issue-type epic

# Create a use case
pebbles new "User can reset password" --issue-type usecase
```

### Linking Issues

```bash
# Make a usecase part of an epic
pebbles link <usecase-id> part_of <epic-id>

# Mark dependencies
pebbles link <issue-id> required_for <other-issue-id>

# Define ordering
pebbles link <issue-id> scheduled_before <other-issue-id>
```

### Filtering

```bash
# List all bugs
pebbles list --issue-type bug

# List blocked issues
pebbles list --status blocked

# List in-progress epics
pebbles list --status in_progress --issue-type epic
```

### Example Workflow

```bash
# Initialize
mkdir my-project && cd my-project
pebbles init

# Create an epic
pebbles new "User Authentication" --issue-type epic
# Output: ✓ Created issue e1a2b3c4

# Create related tasks
pebbles new "Login form design" --issue-type ux
# Output: ✓ Created issue a5b6c7d8

pebbles new "JWT implementation" --issue-type design
# Output: ✓ Created issue f9g0h1i2

# Link them to the epic
pebbles link a5b6 part_of e1a2
pebbles link f9g0 part_of e1a2

# Show the usecase with relationships
pebbles show a5b6
# Shows relationships including "part of e1a2b3c4"

# Check what's in the epic by viewing relationships
pebbles show e1a2
```

## Technical Details

### Dependencies

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
clap = { version = "4.5", features = ["derive", "cargo"] }
anyhow = "1.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.8", features = ["v4", "serde"] }
colored = "2.1"
slug = "0.1"              # For filename generation
fuzzy-matcher = "0.3"     # For fuzzy search (to be implemented)
ratatui = "0.29"          # For TUI (to be implemented)
crossterm = "0.29"        # Terminal handling (to be implemented)
```

### File Naming Algorithm

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

Example outputs:
- `bug-a1b2c3d4-cannot-login.json`
- `ux-e5f6g7h8-dashboard-confusing.json`
- `epic-i9j0k1l2-user-auth.json`

### Type Short Codes

- `bug` → bug
- `ux` → ux_problem
- `uc` → usecase
- `des` → design
- `val` → validation
- `epic` → epic
- `mile` → milestone

## Remaining Features (TODO)

### High Priority

1. **TUI (Terminal User Interface)**
   - Interactive list view with arrow key navigation
   - Visual relationship graph
   - Bulk selection and operations
   - Filtered views (next tasks, route to milestone, overview)

2. **Fuzzy Search**
   - Fuzzy match on titles
   - Search by description text
   - Search by metadata fields

3. **Bulk Operations**
   - Update status of multiple issues
   - Batch link/unlink operations
   - Mass delete filtered issues

### Medium Priority

4. **View Commands**
   - `pebbles graph` - ASCII graph of relationships
   - `pebbles epic <id>` - Show all issues in an epic
   - `pebbles route <from> <to>` - Show dependency path
   - `pebbles next` - Show actionable next tasks

5. **Metadata Management**
   - Commands to set/get metadata fields
   - Type-specific templates (e.g., UX problems should have before/after screenshots)
   - Custom fields support

6. **Search and Filtering**
   - Full-text search across issues
   - Advanced filters (created before/after, updated in last N days)
   - Saved filter sets

### Lower Priority

7. **Semantic Search**
   - Embedding-based similarity search
   - "Find issues similar to this one"
   - Automatic duplicate detection

8. **Export/Import**
   - Export to various formats (JSON, CSV, Markdown)
   - Import from other issue trackers
   - Generate reports

9. **Performance**
   - Caching layer for large repositories
   - Incremental index updates
   - Parallel issue loading

10. **Git Integration**
    - `jj git init --colocate` support
    - Push/pull hooks
    - Branch-aware issue filtering

## Testing

### Manual Test Results ✅

```bash
cd /tmp/pebbles-test2
pebbles init                                          # ✅ Works
pebbles new "Bug" --issue-type bug                    # ✅ Creates bug-xxx-slug.json
pebbles new "Epic" --issue-type epic                  # ✅ Creates epic-xxx-slug.json
pebbles list                                          # ✅ Shows colored types
pebbles link <bug-id> part_of <epic-id>               # ✅ Adds relationship
pebbles show <bug-id>                                 # ✅ Shows relationships
ls .beads/issues/                                     # ✅ Meaningful filenames
jj log                                                # ✅ Descriptive commit messages
```

### Validation Tests ✅

```bash
# Try to make epic part of another epic
pebbles link <epic-id> part_of <other-epic-id>        # ✅ Correctly fails with error
```

## Next Steps

1. **Implement TUI** - Most impactful for usability
2. **Add fuzzy search** - Essential for working with many issues
3. **Bulk operations** - Needed for agent and human efficiency
4. **View commands** - Better project understanding
5. **Documentation updates** - Update README and EXAMPLES with new features

## Breaking Changes

### v0.1.0 → v0.2.0

**Data Format Changes:**
- Index structure changed from `Vec<Uuid>` to `Vec<IssueMetadata>`
- Issue files renamed from `<uuid>.json` to `<type>-<shortid>-<slug>.json`
- Issue struct added `issue_type`, `relationships`, `metadata` fields
- Removed `dependencies` field (replaced by `relationships`)

**Migration:**
Old v0.1.0 repositories need migration:
1. Add `issue_type` to each issue (default to `bug`)
2. Convert `dependencies` to `relationships` with `RequiredFor` type
3. Rename files to new scheme
4. Update index to new format

**Not Implemented:** Automatic migration tool (manual editing required for now)

## Design Decisions

### Why Meaningful Filenames?

**Pros:**
- Human-readable diffs in jj
- Easy to find issues by browsing
- Natural grouping by type
- Grep-friendly

**Cons:**
- Need to handle renames when title changes
- Slightly more complex storage logic

**Decision:** Worth it for better jj integration and developer experience

### Why HashMap for Metadata?

**Pros:**
- Extensible without schema changes
- Type-specific fields without complex enums
- User-defined fields

**Cons:**
- No type safety for metadata values
- Need to document expected fields

**Decision:** Flexibility outweighs type safety for metadata

### Why Separate Relationship Types?

**Pros:**
- Clear semantics
- Type-specific validation
- Better queries (find all epics, find all dependencies)

**Cons:**
- More complex than simple "links"
- Need to choose correct type

**Decision:** Real software projects need these distinctions

## Known Issues

1. **No migration tool** - v0.1.0 repos can't auto-upgrade to v0.2.0
2. **No TUI yet** - Bulk operations are tedious in CLI
3. **No fuzzy search** - Must use exact ID prefixes
4. **File renames** - Old file isn't tracked by jj as a rename, appears as delete+add
5. **No type templates** - UX problems should prompt for screenshots, etc.

## Performance Notes

- Loading all issues: O(n) where n = number of issues
- ID resolution: O(n) linear search (could optimize with HashMap)
- File renames: Two disk operations (delete + write)

For < 1000 issues, performance is negligible. For larger repos, consider caching.

## Conclusion

Pebbles v0.2.0 has successfully implemented the enhanced data model with issue types, relationships, and meaningful filenames. The foundation is solid for building advanced features like TUI, search, and bulk operations.

The design philosophy remains:
- **Simple**: JSON files, no database
- **Reliable**: Explicit validation, no corruption
- **jj-native**: Leverages jj's strengths

Next focus: Build the TUI for better visualization and interaction.
