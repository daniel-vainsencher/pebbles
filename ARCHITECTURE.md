# Pebbles Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         User / AI Agent                          │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      │ CLI Commands
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│                         CLI Layer (cli.rs)                       │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Commands: init, new, list, show, status, update, delete, │  │
│  │            link, unlink                                    │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────┬───────────────────────────────────────────┘
                      │
                      │ Command Routing
                      ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Operations Layer (operations.rs)               │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  Business Logic:                                           │  │
│  │  - CRUD operations                                         │  │
│  │  - Relationship validation                                 │  │
│  │  - ID resolution                                           │  │
│  │  - Filtering                                               │  │
│  └───────────────────────────────────────────────────────────┘  │
└──────────┬──────────────────────────────────────┬───────────────┘
           │                                      │
           │                                      │
           ▼                                      ▼
┌──────────────────────────┐      ┌──────────────────────────────┐
│  Storage (storage.rs)    │      │   Display (display.rs)       │
│  ┌────────────────────┐  │      │  ┌────────────────────────┐  │
│  │ - Load/Save Index  │  │      │  │ - Pretty print list    │  │
│  │ - Load/Save Issues │  │      │  │ - Pretty print details │  │
│  │ - Handle renames   │  │      │  │ - Colored output       │  │
│  └────────────────────┘  │      │  └────────────────────────┘  │
└──────────┬───────────────┘      └──────────────────────────────┘
           │
           ▼
┌──────────────────────────┐      ┌──────────────────────────────┐
│   JJ Integration (jj.rs) │      │   Models (models.rs)         │
│  ┌────────────────────┐  │      │  ┌────────────────────────┐  │
│  │ - Init repo        │  │      │  │ IssueType enum         │  │
│  │ - Describe changes │  │      │  │ RelationType enum      │  │
│  └────────────────────┘  │      │  │ Status enum            │  │
└──────────┬───────────────┘      │  │ Issue struct           │  │
           │                       │  │ Index struct           │  │
           │                       │  └────────────────────────┘  │
           ▼                       └──────────────────────────────┘
┌─────────────────────────────────────────────────────────────────┐
│                      Jujutsu Repository                          │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  .jj/                 (version control metadata)           │  │
│  │  .beads/                                                   │  │
│  │  ├── index.json      (issue metadata: id → filename)      │  │
│  │  └── issues/                                               │  │
│  │      ├── bug-a1b2c3d4-login-broken.json                   │  │
│  │      ├── ux-e5f6g7h8-dashboard-redesign.json              │  │
│  │      ├── epic-i9j0k1l2-user-auth.json                     │  │
│  │      └── uc-m3n4o5p6-user-can-login.json                  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

## Data Flow

### Create Issue Flow

```
User: pebbles new "Title" --issue-type bug
       │
       ▼
CLI: Parse command, extract title, type, description
       │
       ▼
Operations: create(title, desc, type)
       │
       ├─► Create Issue struct with UUID
       │
       ├─► Load Index
       │
       ├─► Storage.save_issue(issue, &mut index)
       │   │
       │   ├─► Generate filename: bug-a1b2c3d4-title.json
       │   ├─► Write JSON file
       │   ├─► Update index with (id, filename)
       │   │
       │   ▼
       ├─► Storage.save_index()
       │
       ├─► JJ.describe("Create bug: Title")
       │   │
       │   └─► jj describe -m "..."
       │
       ▼
Display: print_issue_detail()
```

### List Issues Flow

```
User: pebbles list --status todo --issue-type bug
       │
       ▼
CLI: Parse filters
       │
       ▼
Operations: list(status_filter, type_filter)
       │
       ├─► Storage.load_all_issues()
       │   │
       │   ├─► Load index.json
       │   ├─► For each issue in index:
       │   │   ├─► Read file by name
       │   │   └─► Deserialize JSON → Issue
       │   │
       │   ▼
       ├─► Filter by status
       ├─► Filter by type
       ├─► Sort by updated_at
       │
       ▼
Display: print_issue_list()
```

### Link Issues Flow

```
User: pebbles link <source> part_of <target>
       │
       ▼
CLI: Parse source, relation, target
       │
       ▼
Operations: add_relationship(source, relation, target)
       │
       ├─► Resolve source_id (short ID → UUID)
       ├─► Resolve target_id
       ├─► Load source issue
       ├─► Load target issue
       │
       ├─► source_issue.add_relationship(relation, target_id)
       │   │
       │   ├─► Check for duplicates
       │   ├─► Validate relationship type
       │   │   (e.g., epic can't be part of epic)
       │   ├─► Add to relationships vector
       │   │
       │   ▼
       ├─► Storage.save_issue(source_issue, &mut index)
       ├─► Storage.save_index()
       ├─► JJ.describe("Add relationship: ...")
       │
       ▼
Display: Success message
```

### Update Title Flow (with Rename)

```
User: pebbles update <id> --title "New Title"
       │
       ▼
Operations: update(id, Some(title), None)
       │
       ├─► Resolve id
       ├─► Load issue
       ├─► Update issue.title = "New Title"
       ├─► Update issue.updated_at
       │
       ├─► Storage.save_issue(issue, &mut index)
       │   │
       │   ├─► Generate new_filename: type-id-new-title.json
       │   ├─► Get old_filename from index
       │   ├─► If filenames differ:
       │   │   ├─► Delete old file
       │   │   ├─► Write new file
       │   │   └─► Update index with new filename
       │   │
       │   ▼
       ├─► Storage.save_index()
       ├─► JJ.describe("Update bug: New Title")
       │
       ▼
Display: print_issue_detail()
```

## Module Responsibilities

### main.rs
- Entry point
- Parse CLI arguments
- Create Operations instance
- Route commands to operations
- Handle errors
- Display results

### cli.rs
- Define CLI structure with clap
- Command definitions
- Argument parsing
- Help text

### models.rs
- Data structures:
  - `Issue`: Core issue type
  - `IssueType`: Enum of issue types
  - `RelationType`: Enum of relationship types
  - `Status`: Enum of statuses
  - `Relationship`: Relationship between issues
  - `Index`: Metadata index
  - `IssueMetadata`: Index entry
- String parsing (FromStr implementations)
- Display implementations
- Business logic:
  - `Issue::new()` - Create issue
  - `Issue::update()` - Update timestamp
  - `Issue::file_name()` - Generate filename
  - `Issue::short_id()` - Get 8-char ID
  - `Issue::add_relationship()` - Add with validation
  - `Issue::remove_relationship()` - Remove relationship

### storage.rs
- File I/O operations
- Index management:
  - `load_index()` - Read index.json
  - `save_index()` - Write index.json
- Issue operations:
  - `load_issue(id)` - Read issue by ID
  - `save_issue(issue, index)` - Write issue, handle renames
  - `delete_issue(id, index)` - Delete issue file
  - `load_all_issues()` - Read all issues
- Path resolution:
  - `issue_path(filename)` - Get full path
- Error handling

### operations.rs
- Business logic
- Operations:
  - `init()` - Initialize repository
  - `create()` - Create issue
  - `list()` - List with filtering
  - `show()` - Show single issue
  - `update_status()` - Change status
  - `update()` - Update fields
  - `delete()` - Delete issue
  - `add_relationship()` - Link issues
  - `remove_relationship()` - Unlink issues
- Helpers:
  - `resolve_id()` - Short ID → UUID
- Validation
- Coordination between storage and jj

### jj.rs
- Jujutsu integration
- Commands:
  - `init()` - Initialize jj repo
  - `describe(message)` - Create commit
  - `is_repo()` - Check if jj repo exists
- Command execution
- Error handling

### display.rs
- Terminal output formatting
- Functions:
  - `print_issue_list()` - Format issue list
  - `print_issue_detail()` - Format single issue
- Colored output
- Unicode formatting
- Status and type badges

## Data Model

### Issue JSON Structure

```json
{
  "id": "a1b2c3d4-e5f6-7890-1234-567890abcdef",
  "title": "Login button broken",
  "description": "Button doesn't respond on mobile",
  "status": "todo",
  "issue_type": "Bug",
  "tags": ["mobile", "critical"],
  "relationships": [
    {
      "relation_type": "PartOf",
      "target_id": "12345678-abcd-efgh-ijkl-mnopqrstuvwx"
    },
    {
      "relation_type": "RequiredFor",
      "target_id": "abcdefgh-1234-5678-90ab-cdefghijklmn"
    }
  ],
  "metadata": {
    "priority": "high",
    "assignee": "alice"
  },
  "created_at": "2026-01-06T10:30:00.000000Z",
  "updated_at": "2026-01-06T15:45:00.000000Z"
}
```

### Index JSON Structure

```json
{
  "issues": [
    {
      "id": "a1b2c3d4-e5f6-7890-1234-567890abcdef",
      "file_name": "bug-a1b2c3d4-login-button-broken.json"
    },
    {
      "id": "12345678-abcd-efgh-ijkl-mnopqrstuvwx",
      "file_name": "epic-12345678-user-authentication.json"
    }
  ]
}
```

## Validation Rules

### Issue Type Validation
- All types can be created independently
- Type affects available operations (e.g., part_of validation)

### Relationship Validation

#### PartOf Relationships
```rust
// Allowed to be part of epic:
Bug, UxProblem, Usecase, Design, Validation

// NOT allowed to be part of epic:
Epic, Milestone

// Validation in Issue::add_relationship()
if relation_type == RelationType::PartOf {
    match self.issue_type {
        IssueType::Epic | IssueType::Milestone => {
            return Err("Epic and Milestone cannot be part of an epic");
        }
        _ => {}
    }
}
```

#### Duplicate Prevention
- Can't add the same relationship twice
- Checked in `add_relationship()`

### File Naming

```rust
pub fn file_name(&self) -> String {
    let slug = slug::slugify(&self.title);
    let short_slug = if slug.len() > 40 {
        &slug[..40]
    } else {
        &slug
    };
    format!("{}-{}-{}.json", 
        self.issue_type.short_code(), 
        self.short_id(), 
        short_slug)
}
```

**Examples:**
- `bug-a1b2c3d4-login-button-broken.json`
- `ux-e5f6g7h8-dashboard-redesign.json`
- `epic-i9j0k1l2-user-authentication.json`
- `uc-m3n4o5p6-user-can-login-with-email.json`

## Error Handling

### Strategy
- Use `anyhow::Result` throughout
- Context at each layer
- User-friendly error messages
- No panics in normal operation

### Error Propagation

```rust
// Storage layer
pub fn load_issue(&self, id: &Uuid) -> Result<Issue> {
    let index = self.load_index()?;
    let file_name = index.get_file_name(id)
        .ok_or_else(|| anyhow::anyhow!("Issue not found in index: {}", id))?;
    let path = self.issue_path(file_name);
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read issue file: {}", file_name))?;
    let issue: Issue = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse issue file: {}", file_name))?;
    Ok(issue)
}

// Operations layer adds more context
pub fn show(&self, id: &str) -> Result<Issue> {
    if !self.storage.exists() {
        bail!("Pebbles not initialized. Run 'pebbles init' first.");
    }
    let uuid = self.resolve_id(id)?;
    self.storage.load_issue(&uuid)
}
```

## Performance Considerations

### Current Performance
- **Storage:** O(1) file read/write
- **Index lookup:** O(n) linear search
- **List operations:** O(n) load all issues
- **ID resolution:** O(n) prefix matching
- **Acceptable for:** < 1000 issues

### Future Optimizations
- HashMap for index (O(1) lookup)
- Incremental loading
- Caching layer
- Parallel file operations

## Extension Points

### Adding New Issue Types
1. Add variant to `IssueType` enum
2. Add to `FromStr` implementation
3. Add short code to `short_code()`
4. Add color to display.rs
5. Update validation rules if needed

### Adding New Relationship Types
1. Add variant to `RelationType` enum
2. Add to `FromStr` implementation
3. Add validation in `add_relationship()`
4. Add display string in display.rs

### Adding New Commands
1. Add variant to `Commands` enum in cli.rs
2. Add operation method in operations.rs
3. Add command handler in main.rs
4. Update display if needed

### Adding Custom Fields
- Use `metadata` HashMap
- No code changes required
- Document expected fields per type

## Testing Strategy

### Current State
- Manual testing of all features
- Integration testing via demo scripts
- No unit tests yet

### Recommended Tests
```rust
#[cfg(test)]
mod tests {
    // Unit tests for models
    #[test]
    fn test_issue_file_name() { }
    
    #[test]
    fn test_relationship_validation() { }
    
    // Integration tests for operations
    #[test]
    fn test_create_and_load() { }
    
    #[test]
    fn test_link_validation() { }
}
```

## Security Considerations

### File System
- Issues stored as plain text JSON
- No encryption (could add if needed)
- Standard file permissions

### Input Validation
- All user input parsed and validated
- UUID validation
- Relationship validation
- Type validation

### Command Injection
- All jj commands use proper argument passing
- No shell evaluation of user input

## Future Architecture Improvements

### v0.3.0 - TUI
```
┌─────────────────────┐
│   Ratatui Layer     │
│  ┌───────────────┐  │
│  │ List View     │  │
│  │ Detail View   │  │
│  │ Graph View    │  │
│  └───────────────┘  │
└──────────┬──────────┘
           │
    (reuse existing layers)
```

### v0.4.0 - Search
```
┌─────────────────────┐
│   Search Layer      │
│  ┌───────────────┐  │
│  │ Fuzzy Match   │  │
│  │ Full Text     │  │
│  │ Semantic      │  │
│  └───────────────┘  │
└──────────┬──────────┘
           │
    (reuse existing layers)
```

### v1.0.0 - Multi-Repo
```
┌─────────────────────┐
│  Repository Manager │
│  ┌───────────────┐  │
│  │ Multi-repo    │  │
│  │ Sync          │  │
│  │ Aggregation   │  │
│  └───────────────┘  │
└──────────┬──────────┘
           │
    (multiple storage instances)
```

## Summary

Pebbles has a clean, layered architecture with clear separation of concerns:
- **CLI** for user interaction
- **Operations** for business logic
- **Storage** for persistence
- **Models** for data structures
- **Display** for output formatting
- **JJ** for version control

This architecture makes it easy to:
- Add new features
- Test components
- Extend functionality
- Maintain code quality

The design prioritizes simplicity and reliability over complexity.
