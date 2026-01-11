# Pebbles Specification

This document specifies the data model, file formats, and behavior of Pebbles.

## Data Model

The complete data model is defined in [`src/models.rs`](src/models.rs). Key types:

### Issue

An issue represents a unit of work tracked by Pebbles. See [`Issue` struct](src/models.rs#L139-L151) for the complete definition.

**Core fields:**
- `id` - UUID v4 identifier
- `title` - Human-readable summary
- `description` - Optional detailed description
- `status` - Current status (see [Status](#status))
- `issue_type` - Type of issue (see [Issue Types](#issue-types))
- `relationships` - Links to other issues (see [Relationships](#relationships))
- `tags` - List of string tags (currently not used by CLI)
- `metadata` - Extensible key-value storage for custom fields
- `created_at` / `updated_at` - Timestamps in UTC

### Status

Four statuses represent the lifecycle of an issue. See [`Status` enum](src/models.rs#L6-L13).

- `todo` - Not started
- `in_progress` - Currently being worked on
- `blocked` - Cannot proceed due to external dependency
- `done` - Completed

### Issue Types

Seven types classify issues by their nature. See [`IssueType` enum](src/models.rs#L40-L50).

- `bug` - Software defects requiring fixes
- `ux_problem` - User experience issues
- `usecase` - User stories or requirements
- `design` - Software design or architecture problems
- `validation` - Validation features or checks
- `epic` - Container for grouping related issues
- `milestone` - Ordering or prioritization marker

**File naming short codes:**
- `bug` → bug
- `ux_problem` → ux
- `usecase` → uc
- `design` → des
- `validation` → val
- `epic` → epic
- `milestone` → mile

### Relationships

Issues can be linked via relationships. See [`Relationship` struct](src/models.rs#L132-L136) and [`RelationType` enum](src/models.rs#L101-L107).

**Relationship types:**
- `part_of` - Source issue is part of target (typically an epic)
- `required_for` - Source must be completed before target
- `scheduled_before` - Source should be completed before target

### Index

The index maps issue IDs to their current filenames. See [`Index` struct](src/models.rs#L234-L237) and [`IssueMetadata` struct](src/models.rs#L228-L232).

## File Format

### Directory Structure

```
.pebbles/
├── index.json
└── issues/
    ├── {type}-{short_id}-{slug}.json
    ├── {type}-{short_id}-{slug}.json
    └── ...
```

### Index File

**Path:** `.pebbles/index.json`

**Format:**
```json
{
  "issues": [
    {
      "id": "uuid-v4",
      "file_name": "type-shortid-slug.json"
    },
    ...
  ]
}
```

The index maintains the mapping between UUIDs and current filenames. When an issue's title changes, the filename changes but the ID remains constant.

### Issue Files

**Path:** `.pebbles/issues/{type}-{short_id}-{slug}.json`

**Naming:**
- `{type}` - Issue type short code (see [Issue Types](#issue-types))
- `{short_id}` - First 8 characters of UUID
- `{slug}` - URL-safe slug of title (max 40 chars)

**Format:** See [`Issue` struct serialization](src/models.rs#L139-L151)

**Example:**
```json
{
  "id": "a1b2c3d4-e5f6-7890-1234-567890abcdef",
  "title": "Login button broken",
  "description": "Button doesn't respond on mobile",
  "status": "todo",
  "issue_type": "bug",
  "tags": [],
  "relationships": [
    {
      "relation_type": "part_of",
      "target_id": "e5f6g7h8-i9j0-k1l2-m3n4-o5p6q7r8s9t0"
    }
  ],
  "metadata": {},
  "created_at": "2026-01-10T12:00:00.000000Z",
  "updated_at": "2026-01-10T12:00:00.000000Z"
}
```

## Validation Rules

### Relationship Validation

**`part_of` relationships:**
- Only `bug`, `ux_problem`, `usecase`, `design`, and `validation` types can be part of an epic
- `epic` and `milestone` types cannot be part of another epic
- Implementation: [`IssueType::can_be_part_of_epic()`](src/models.rs#L84-L86)

**Duplicate prevention:**
- Cannot add the same relationship (same type and target) twice
- Implementation: [`Issue::add_relationship()`](src/models.rs#L188-L207)

### ID Resolution

**Short IDs:**
- Minimum 1 character, recommended 4+ characters
- Must match exactly one issue's UUID prefix
- Case-insensitive
- Implementation: [`Operations::resolve_id()`](src/operations.rs)

## CLI Interface

The complete CLI specification is defined in [`src/cli.rs`](src/cli.rs).

### Commands

- `init` - Initialize pebbles repository
- `new <title>` - Create new issue
- `list` - List all issues with optional filters
- `show <id>` - Show issue details
- `status <id> <status>` - Update issue status
- `update <id>` - Update issue title or description
- `delete <id>` - Delete issue
- `link <source> <relation> <target>` - Create relationship
- `unlink <source> <relation> <target>` - Remove relationship

See [`Commands` enum](src/cli.rs#L11-L93) for complete argument specifications.

## Behavior

### File Renaming

When an issue's title is updated:
1. New filename is generated based on new title
2. Old file is deleted
3. New file is created with updated content
4. Index is updated with new filename
5. Jj tracks this as a delete + add (not a rename)

Implementation: [`Storage::save_issue()`](src/storage.rs)

### Timestamp Updates

The `updated_at` timestamp is automatically updated when:
- Issue status changes
- Issue title or description changes
- Relationships are added or removed

Implementation: [`Issue::update()`](src/models.rs#L170-L172)

### Jujutsu Integration

Every mutating operation creates a jj change with a descriptive message:
- Create: "Create {type}: {title}"
- Update status: "Update issue {title}: {old_status} → {new_status}"
- Update fields: "Update {type}: {title}"
- Delete: "Delete {type}: {title}"
- Link: "Add relationship: {source} {relation} {target}"
- Unlink: "Remove relationship: {source} {relation} {target}"

Implementation: [`JJ::describe()`](src/jj.rs)

## Extension Points

### Custom Metadata

The `metadata` field is a `HashMap<String, String>` that can store arbitrary key-value pairs. This allows for:
- Type-specific fields without schema changes
- User-defined attributes
- Tool-specific data

**Access:** Currently only exposed through direct JSON manipulation. CLI commands for metadata management are not yet implemented.

### Tags

The `tags` field is a `Vec<String>` for categorization. Currently not used by CLI commands but available in the data model for future features or direct JSON manipulation.

## Future Considerations

Features mentioned in historical documentation but not yet implemented:
- Tag management CLI commands
- Metadata management CLI commands
- TUI (Terminal User Interface)
- Fuzzy search
- Bulk operations
- View commands (graph, route, next, epic)
- Full-text search
- Export/import functionality
- Templates per issue type

These features are tracked as pebbles issues in the repository.
