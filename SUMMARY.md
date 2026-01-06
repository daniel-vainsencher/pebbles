# Pebbles v0.2.0 - Implementation Summary

## Overview

Pebbles is now a fully-functional, rock-solid issue management tool built on Jujutsu (jj) with enhanced features for both humans and AI agents. Version 0.2.0 represents a significant upgrade from the basic v0.1.0, adding issue types, relationships, and meaningful file naming.

## What We Built

### Core Architecture (v0.1.0)
- **Rust-based CLI** using clap for parsing
- **JSON storage** in `.beads/` directory
- **Jujutsu integration** for version control
- **CRUD operations** for issues
- **Short ID support** for easy reference
- **Colored terminal output** for better UX

### Enhanced Features (v0.2.0)
- **7 Issue Types** with validation and colored display
- **3 Relationship Types** with semantic validation rules
- **Meaningful File Names** (type-shortid-slug.json)
- **Automatic file renaming** when titles change
- **Link/Unlink commands** for managing relationships
- **Metadata support** for extensibility
- **Blocked status** for tracking blockers
- **Type-based filtering** in list command

## File Structure

```
pebbles/
├── src/
│   ├── main.rs          # CLI entry point, command routing
│   ├── cli.rs           # Command definitions with clap
│   ├── models.rs        # Issue, IssueType, RelationType, Status, Index
│   ├── storage.rs       # File I/O with rename handling
│   ├── jj.rs            # Jujutsu command execution
│   ├── operations.rs    # Business logic, validation, CRUD
│   └── display.rs       # Pretty terminal output with colors
├── Cargo.toml           # Dependencies and metadata
├── README.md            # User documentation
├── QUICKSTART.md        # Quick reference guide
├── EXAMPLES.md          # Usage examples
├── DESIGN.md            # Architecture documentation
├── PROGRESS.md          # Development status and roadmap
├── CHANGELOG.md         # Version history
├── SUMMARY.md           # This file
└── LICENSE              # MIT license

.beads/
├── index.json           # Issue metadata (id → filename mapping)
└── issues/
    ├── bug-a1b2c3d4-login-button-broken.json
    ├── ux-e5f6g7h8-dashboard-redesign.json
    ├── epic-i9j0k1l2-user-authentication.json
    └── uc-m3n4o5p6-user-can-login.json
```

## Key Design Decisions

### 1. Meaningful File Naming
**Decision:** Use `type-shortid-slug.json` instead of `<uuid>.json`

**Benefits:**
- Human-readable jj diffs
- Natural grouping by type
- Easy browsing and grep
- Better understanding of project state

**Implementation:**
- `Issue::file_name()` generates names
- `Index` tracks id → filename mapping
- Storage handles renames automatically
- Old file deleted, new file created (jj sees as delete+add)

### 2. Relationship Validation
**Decision:** Enforce type-specific rules for relationships

**Rules:**
- Only bug, ux, usecase, design, validation can be `part_of` an epic
- Epics cannot be part of other epics
- No duplicate relationships

**Benefits:**
- Prevents nonsensical relationships
- Clear semantics
- Better project structure

### 3. Multiple Relationship Types
**Decision:** Three explicit types instead of generic "links"

**Types:**
- `part_of` - Hierarchical grouping
- `required_for` - Dependencies
- `scheduled_before` - Ordering

**Benefits:**
- Clear semantics for different use cases
- Better queries and filtering (future)
- Supports different workflows

### 4. Metadata as HashMap
**Decision:** Free-form key-value storage

**Benefits:**
- Extensible without schema changes
- Type-specific custom fields
- User-defined attributes

**Trade-offs:**
- No type safety for values
- Need documentation for expected fields

## Technical Implementation

### Data Model

```rust
pub enum IssueType {
    Bug, UxProblem, Usecase, Design, Validation, Epic, Milestone
}

pub enum RelationType {
    PartOf, RequiredFor, ScheduledBefore
}

pub struct Relationship {
    pub relation_type: RelationType,
    pub target_id: Uuid,
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

### Storage Layer

**Read Flow:**
1. Load index.json
2. Look up filename by UUID
3. Read issue file
4. Deserialize JSON

**Write Flow:**
1. Generate new filename from issue
2. Check if filename changed
3. Delete old file if renamed
4. Write new file
5. Update index
6. Save index
7. Create jj commit

**Delete Flow:**
1. Load index
2. Look up filename
3. Delete file
4. Remove from index
5. Save index
6. Create jj commit

### Validation Layer

**Relationship Validation:**
```rust
impl Issue {
    pub fn add_relationship(&mut self, relation_type: RelationType, target_id: Uuid) 
        -> Result<(), String> 
    {
        // Check for duplicates
        if self.relationships.iter()
            .any(|r| r.relation_type == relation_type && r.target_id == target_id) 
        {
            return Err("Relationship already exists".to_string());
        }

        // Validate part_of relationships
        if relation_type == RelationType::PartOf {
            match self.issue_type {
                IssueType::Epic | IssueType::Milestone => {
                    return Err("Epic and Milestone cannot be part of an epic".to_string());
                }
                _ => {}
            }
        }

        self.relationships.push(Relationship { relation_type, target_id });
        Ok(())
    }
}
```

### Display Layer

**List View:**
- Short ID (8 chars)
- Type badge with color
- Status badge with color
- Title in bold

**Detail View:**
- Full UUID
- All fields including relationships
- Metadata if present
- Timestamps
- Current filename

## Testing

All features tested with:
- Unit operations (create, update, delete)
- Relationship creation and validation
- File naming and renaming
- Type and status filtering
- Short ID resolution
- Jj integration

**Test Results:** ✅ All passing

## Dependencies

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
clap = { version = "4.5", features = ["derive", "cargo"] }
anyhow = "1.0"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.8", features = ["v4", "serde"] }
colored = "2.1"
slug = "0.1"
fuzzy-matcher = "0.3"  # Future: fuzzy search
ratatui = "0.29"        # Future: TUI
crossterm = "0.29"      # Future: TUI
```

## Performance Characteristics

- **Storage:** O(1) file read/write
- **Index lookup:** O(n) linear search (could optimize with HashMap)
- **List operations:** O(n) where n = number of issues
- **ID resolution:** O(n) prefix matching

For repositories with < 1000 issues, performance is negligible.

## Known Limitations

1. **No migration tool** - v0.1.0 → v0.2.0 requires manual migration
2. **No TUI yet** - CLI only
3. **No fuzzy search** - Exact ID prefix matching only
4. **Linear index search** - Could be optimized with HashMap
5. **File renames** - Appear as delete+add in jj, not tracked as rename
6. **No batch operations** - One issue at a time

## Future Roadmap

### Near Term (v0.3.0)
- TUI with ratatui
- Fuzzy search for titles
- Bulk operations
- View commands (graph, route, next)

### Medium Term (v0.4.0)
- Semantic search
- Custom templates per type
- Export/import
- Performance optimizations

### Long Term (v1.0.0)
- Multi-repository support
- Web interface
- Integration with other tools
- Advanced analytics

## Success Metrics

✅ **Stability:** Zero data corruption, no crashes in testing
✅ **Usability:** Clear, intuitive CLI
✅ **Jj Integration:** Meaningful commits, leverages jj features
✅ **Extensibility:** Easy to add new features
✅ **Performance:** Fast for typical repositories (< 1000 issues)
✅ **Documentation:** Comprehensive guides and examples

## Conclusion

Pebbles v0.2.0 successfully achieves the goal of being a rock-solid, helpful issue management tool built on Jujutsu. The enhanced data model with issue types, relationships, and meaningful filenames provides a strong foundation for future features while maintaining simplicity and reliability.

The tool is production-ready for:
- Personal project management
- Small team collaboration (with shared jj repository)
- AI agent task tracking
- Development workflow organization

Next steps focus on improving the user experience with TUI, search, and bulk operations while maintaining the core principle: **stable as a rock**.

---

**Built with Rust and Jujutsu**
**Version:** 0.2.0
**Date:** 2026-01-06
**License:** MIT
