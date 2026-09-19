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

Five statuses represent the lifecycle of an issue. See [`Status` enum](src/models.rs#L6-L14).

- `proposed` - Created and awaiting approval before work starts
- `todo` - Approved and not started
- `in_progress` - Currently being worked on
- `blocked` - Cannot proceed due to external dependency
- `done` - Completed

r[status-serialization]
On disk, `status` is stored with the canonical spelling shown above (`in_progress`, not `inprogress`). Older issue files containing `"inprogress"` still load — it is accepted as a legacy alias on read.
Implementation: [`Status` enum](src/models.rs) | Verify: [models tests](src/models.rs)

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
- `required_for` - Source must be completed before target (hard blocker)
- `scheduled_before` - Source should be completed before target (hard blocker)

**Relationship storage:**
Relationships are stored only on the source issue. For example, `pebbles link A required_for B` adds a `required_for` entry to A's file pointing to B; B's file has no corresponding entry.

**Incoming relationships:**
To determine what blocks an issue, use the incoming relationship map which inverts this storage model. The pure function [`build_incoming_relationships_map()`](src/models.rs) builds a map from target ID to all relationships pointing at it, enabling efficient lookup of predecessors. This is used by `next` to filter blocked issues and by `show` to display incoming relationships.

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

r[issue-filename-format]
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

## Repository Graph Integrity

The issue set forms a graph: nodes are issues, edges are relationships. These
invariants apply to the graph as a whole, in addition to the per-issue
validation rules below. They are enforced when relationships change and can
be checked on demand over the whole repository.

r[graph-acyclic]
**Acyclic dependency graph (DAG):**
- The directed graph over `required_for` and `scheduled_before` edges must remain acyclic.
- A `link` that would close a directed cycle is rejected, including the minimal cycle of a self-edge.
- Implementation: [`Operations::add_relationship()`](src/operations.rs), [`Operations::convert()`](src/operations.rs) | Test: [jj-integration](tests/jj_integration.rs)

## Validation Rules

### Relationship Validation

r[part-of-epic-types]
**`part_of` relationships:**
- Only `bug`, `ux_problem`, `usecase`, `design`, and `validation` types can be part of an epic
- `epic` and `milestone` types cannot be part of another epic
- Example: [Quick Start](README.md#link-issues) demonstrates `part_of` relationships
- Test: [core-workflow](tests/workflows/core-workflow.trycmd#link-issues-to-epic) shows epic linking
- Implementation: [`IssueType::can_be_part_of_epic()`](src/models.rs#L84-L86)

r[no-duplicate-relationships]
**Duplicate prevention:**
- Cannot add the same relationship (same type and target) twice
- Implementation: [`Issue::add_relationship()`](src/models.rs#L188-L207)

**Self-relationships:**
- An issue cannot be linked to itself; `pebbles link <id> <relation> <id>` is an error
- This is the minimal case of the acyclicity invariant r[graph-acyclic]
- Test: [jj-integration](tests/jj_integration.rs) t12_self_link_rejected

### ID Resolution

r[appropriate-length-id]
**Appropriate Length ID:**

The "appropriate length ID" is the canonical way to display issue IDs in pebbles output. It is defined as:

- The **minimum-length prefix** of the UUID that is both:
  1. **Unambiguous** - matches exactly one issue in the database
  2. **At least 3 characters** - to be clearly recognizable as an ID
- Always **lowercase**
- Used in creation messages (e.g., `Created bug 763`) and recommended for user display

Implementation: [`Operations::appropriate_length_id()`](src/operations.rs)

r[short-id-input]
**Short ID Input:**
- Minimum 3 characters when entering IDs[^1]
- Must match exactly one issue's UUID prefix
- Case-insensitive on input, normalized to lowercase
- Example: [Quick Start](README.md#view-details) shows short ID usage
- Test: [core-workflow](tests/workflows/core-workflow.trycmd#show-issue-details) demonstrates short IDs
- Implementation: [`Operations::resolve_id()`](src/operations.rs)

[^1]: The minimum ID length may need to increase as more issues are created and ID collisions become more likely.

## CLI Interface

The complete CLI specification is defined in [`src/cli.rs`](src/cli.rs).

### Commands

- `init` - Initialize pebbles repository  
  Example: [Quick Start](README.md#initialize)

r[new-issues-default-proposed]
- `new <title>` - Create new issue with optional `--issue-type` and `-d` for description  
  New issues start with status `proposed`. Passing `--approved` creates the issue with status `todo` instead.
  The title must be non-empty after trimming; empty or whitespace-only titles are an error.
  Example: [Quick Start](README.md#create-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#create-a-new-issue) | Review workflow: [review-workflow](tests/workflows/review-workflow.trycmd#review-proposed-issues)

r[list-filter]
- `list` - List all issues with optional `--status` and `--issue-type` filters  
  Example: [Quick Start](README.md#list-and-filter) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#list-all-issues)

- `show <id>` - Show issue details  
  Example: [Quick Start](README.md#view-details) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#show-issue-details)

- `status <id> <status>` - Update issue status  
  Example: [Quick Start](README.md#update-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#update-issue-status)

r[approve-command]
- `approve <id>` - Approve a proposed issue  
  Moves an issue from `proposed` to `todo` and is equivalent to `status <id> todo` with a proposed-only guard: approving a non-proposed issue is an error.
  Test: [review-workflow](tests/workflows/review-workflow.trycmd#approve-a-proposed-issue) | Verification: [jj-integration](tests/jj_integration.rs) t10_approve

- `update <id>` - Update issue title with `--title` or description with `--description`  
  Example: [Quick Start](README.md#update-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#update-issue-title)

- `delete <id>` - Delete issue  
  Example: [Quick Start](README.md#delete-issues)

- `link <source> <relation> <target>` - Create relationship (`part_of`, `required_for`, `scheduled_before`)  
  Example: [Quick Start](README.md#link-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#link-issues-to-epic)

- `unlink <source> <relation> <target>` - Remove relationship  
  Example: [Quick Start](README.md#link-issues)

- `epic <id>` - Show epic with all child issues  
  Test: [core-workflow](tests/workflows/core-workflow.trycmd#view-epic-with-all-child-issues)

- `next` - Show actionable TODO issues with no blockers  
  Returns issues where `status == todo` and no incomplete predecessors exist.
  An issue is blocked if it has an incoming `required_for` or `scheduled_before` relationship from a non-Done issue.
  Implementation: [`Operations::next()`](src/operations.rs), [`build_incoming_relationships_map()`](src/models.rs)

r[review-command]
- `review` - List proposed issues awaiting review  
  Returns only issues with status `proposed`, in the same format and ordering as `list` (sorted by `updated_at` descending).
  Approving an issue is done with `status <id> todo`.
  Test: [review-workflow](tests/workflows/review-workflow.trycmd#review-proposed-issues)

- `summary` - Show project summary and statistics

See [`Commands` enum](src/cli.rs#L11-L99) for complete argument specifications.

## Behavior

### Issue Ordering

r[issue-ordering]
When displaying multiple issues (`list`, `epic`, `next`), issues are sorted by `updated_at` timestamp in descending order (most recently updated first). This means:
- Recently modified issues appear at the top
- Marking an issue done moves it to the top (since that updates the timestamp)
- The order reflects recent activity, not status or priority

Implementation: [`Operations::list()`](src/operations.rs), [`Operations::epic()`](src/operations.rs), [`Operations::next()`](src/operations.rs)

### File Renaming

r[rename-commits-all-paths]
When an issue's title is updated (and analogously when its type is converted):
1. New filename is generated based on new title (or new type)
2. Old file is deleted
3. New file is created with updated content
4. Index is updated with new filename
5. Jj records all three changes (old-file deletion, new-file addition, index modification) in the same commit. Whether jj's `diff --summary` displays the file movement as `R {old => new}` or as `D old` + `A new` depends on its similarity heuristic and is not specified here.

Example: Test: [core-workflow](tests/workflows/core-workflow.trycmd#update-issue-title) shows title update triggering filename change
Verification: [jj-integration](tests/jj_integration.rs) tests `t5_rename_via_title` and `t8_convert` confirm all three paths land in the same commit
Implementation: [`Storage::save_issue()`](src/storage.rs)

### Timestamp Updates

The `updated_at` timestamp is automatically updated when:
- Issue status changes
- Issue title or description changes
- Relationships are added or removed

Implementation: [`Issue::update()`](src/models.rs#L170-L172)

### Jujutsu Integration

Every mutating operation commits the changed files to jj with a descriptive message:
- Create: "Create {type}: {title}"
- Update status: "Update {type} {title}: {old_status} → {new_status}"
- Update fields: "Update {type}: {title}"
- Delete: "Delete {type}: {title}"
- Link: "Add relationship: {source} {relation} {target}"
- Unlink: "Remove relationship: {source} {relation} {target}"
- Convert type: "Convert {title}: {old_type} → {new_type}"

r[jj-commit-complete-path-set]
Each operation runs `jj commit -m "<message>" <paths...>`, passing the **complete** set of paths the operation affected. This is the contract: every path pebbles modified, deleted, or created in service of the operation must be in the commit, so the working copy is clean afterward and nothing pebbles changed outlives the commit. Concretely, the path set always includes the affected issue file(s), and additionally:

- `.pebbles/index.json` whenever the index was modified — i.e., on `create`, `delete`, and any operation that renames a file (title update, type conversion).
- The old issue file path (now a deletion on disk) on any rename.

The path set does *not* include unrelated working-copy changes; jj's path-restricted commit confines each pebbles operation to its own changes only.

Implementation: [`JJ::commit()`](src/jj.rs), [`Operations::paths_for_save()`](src/operations.rs)
Verification: [jj-integration](tests/jj_integration.rs) — every mutating operation has a test asserting (a) clean working copy afterward, (b) no paths committed outside `.pebbles/`, and (c) the most recent commit's affected-path set matches the expected set exactly.

r[no-jj-skip]
**Test mode:** If a `.no_jj` marker file exists in the pebbles directory, all jj operations are skipped. This enables safe testing without requiring a jj repository, but bypasses the integration entirely; the [jj-integration](tests/jj_integration.rs) suite deliberately runs *without* this marker against a temporary repo.

## Extension Points

### Custom Metadata

The `metadata` field is a `HashMap<String, String>` that can store arbitrary key-value pairs. This allows for:
- Type-specific fields without schema changes
- User-defined attributes
- Tool-specific data

**Access:** The CLI is the preferred way to update issues (see [CLI Interface](#cli-interface)); dedicated metadata commands are not yet implemented. Prefer scripting with `jq` for metadata. Edit JSON manually only if the CLI is failing — manual edits bypass pebbles' validation rules and jj commits.

### Tags

The `tags` field is a `Vec<String>` for categorization. Currently not used by CLI commands. Use the CLI for all supported updates; edit JSON manually only if the CLI is failing.

## Future Considerations

Features mentioned in historical documentation but not yet implemented:
- Tag management CLI commands
- Metadata management CLI commands
- TUI (Terminal User Interface)
- Fuzzy search
- Bulk operations
- View commands (graph, route)
- Full-text search
- Export/import functionality
- Templates per issue type

These features are tracked as pebbles issues in the repository.
