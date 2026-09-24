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
- `tags` - List of string tags for categorization (see [Tags](#tags))
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

r[issues-dir-created-on-demand]
**Issues directory created on demand:**
- `.pebbles/issues/` is recreated on demand whenever an issue file is written, so a missing directory never aborts a mutation. The directory can disappear on its own: jj (like other VCSs) tracks files, not directories, so removing the last issue file deletes the directory, and so does checking out a commit from before issues existed.
- Implementation: [`Storage::save_issue()`](src/storage.rs) | Test: [jj-integration](tests/jj_integration.rs)

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

r[index-write-atomicity]
**Atomic index writes:**
- Every index write goes through a temp file in the same directory plus a rename, so readers never observe a torn `index.json`.
- Implementation: [`Storage::save_index()`](src/storage.rs) | Test: [jj-integration](tests/jj_integration.rs)

r[stale-index-warning]
**Write-time stale-index detection:**
- At mutation write time, pebbles scans `.pebbles/issues/` for `*.json` files newer than the index version read (baseline: the index file's mtime, not the clock) and not mapped by that index, excluding the current operation's own files.
- When any are found, the operation prints a warning that the index is stale and should be regenerated with `pebbles reindex`, and continues writing (no refusal). A newer file the index already maps is not staleness: the `id -> file_name` entry is still correct.
- Implementation: [`Storage::stale_index_warning()`](src/storage.rs) | Test: [storage](src/storage.rs)

r[reindex]
**`pebbles reindex` (repair command):**
- Rebuilds `.pebbles/index.json` from the issue files: one entry per file with a parseable UUID `id` (first file wins per id), never touching issue files.
- Entry order is preserved from the current index when it parses; otherwise entries follow scan order (alphabetical by file name), i.e. deterministic.
- The rebuilt index is written atomically and verified with a load before the write; a store whose rebuilt index equals the current one is a no-op.
- Unparsable files and files duplicating an id are reported, not indexed.
- The command never runs jj: after a merge or repair, the rebuilt index is left in the working copy for the owner to commit.
- Implementation: [`Storage::reindex()`](src/storage.rs) | Test: [jj-integration](tests/jj_integration.rs) t30, [cmd/reindex](tests/cmd/reindex.trycmd)

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

r[no-broken-links]
**No broken links:**
- Every relationship `target_id` must reference a live issue.
- Deleting an issue that other issues reference is an error naming each broken link, unless `--delete-links-too` is passed, in which case the referencing relationships are removed in the same operation.
- Implementation: [`Operations::delete()`](src/operations.rs) | Test: [jj-integration](tests/jj_integration.rs)

r[index-consistent]
**Index consistency:**
- `.pebbles/index.json` must map exactly the set of files under `.pebbles/issues/`: every index entry resolves to an issue file, and every issue file is indexed.
- Divergence (missing files, orphaned files, unknown entries) is detectable on demand and reported by `pebbles doctor` (r[doctor]); the repair half is `pebbles reindex` (r[reindex]).
- Implementation: [`doctor()`](src/operations.rs) | Test: [operations unit tests](src/operations.rs), [doctor workflow](tests/workflows/doctor.trycmd)

r[doctor]
**`pebbles doctor` (diagnostic command):**
- Reports divergence between `.pebbles/index.json` and `.pebbles/issues/` without touching the store. Reports only; the repair half is `pebbles reindex` (r[reindex]).
- Four divergence classes, reported with a per-class summary and one line per divergence: `missing` (index entry whose file is not on disk), `orphaned` (issue file on disk the index does not map), `unknown` (entry whose file does not parse as an issue; an unparsable index - e.g. an entry whose id is not a UUID - is reported as a store-level `unknown` on `index.json`), `mismatch` (indexed file that parses as an issue declaring a different id than the entry maps).
- Loads the issue set at most once per invocation (one read pass over the indexed files; orphan files are only listed, never parsed).
- Output is deterministic (diagnostics sorted by file name, class blocks in fixed order) and machine-greppable (a `class (count):` header followed by one indented `file (detail)` line per diagnostic).
- Exit status is 0 when the store is consistent and non-zero when any divergence is found.
- Implementation: [`doctor()`](src/operations.rs) | Test: [operations unit tests](src/operations.rs), [doctor workflow](tests/workflows/doctor.trycmd)

r[schema-valid-issues]
**Schema-valid issue files:**
- Every file under `.pebbles/issues/` must parse as a schema-compliant `Issue`.
- Files that fail to parse or violate the schema are reported when the issue set is loaded, and never silently ignored.
- **Unknown keys are schema violations.** A file whose top-level keys go beyond the `Issue` schema (`id`, `title`, `description`, `status`, `issue_type`, `tags`, `relationships`, `metadata`, `created_at`, `updated_at`) is loaded with a warning on every load naming the file and the keys; the issue stays usable and commands exit 0 (degraded-but-usable). There is deliberately no passthrough or migration machinery: a later rewrite serializes only the parsed fields, so the unknown keys are dropped - the load warning makes that consequence explicit in advance. Fix the file to remove them.
- A malformed file (unparsable JSON, missing required field, invalid value) is reported and the issue is absent from the loaded set (the set is degraded).
- Callers can distinguish a degraded issue set from a clean one: `load_all_issues` returns the issues plus a `Vec<LoadWarning>`; an empty vector means every indexed file loaded cleanly. The CLI prints each warning through its own conventions (`Warning: ...` lines); the TUI folds them into its message and detail lines instead of raw stderr (r[load-warnings-as-data]).
- Implementation: [`Storage::load_all_issues()`](src/storage.rs) | Test: [cmd/reindex](tests/cmd/reindex.trycmd), [storage](src/storage.rs)

r[load-warnings-as-data]
**Load warnings are data:**
- Loading the issue set never writes to stderr. `load_all_issues` returns per-item outcomes (`Vec<Issue>`, `Vec<LoadWarning>`); the CLI prints `Warning: ...` lines through its own conventions, and the TUI folds them into its message row and project detail lines instead of raw stderr, which would corrupt the raw-mode alternate screen.
- Implementation: [`Storage::load_all_issues()`](src/storage.rs) | Test: [cmd/reindex](tests/cmd/reindex.trycmd), [tui-e2e](tests/tui_e2e.rs)

r[store-without-jj]
**Store without a jj repository:**
- A mutation that writes the store successfully exits 0 even when jj cannot commit because there is no jj repository; the fact that the change was not committed to version control is surfaced as a warning. Exit codes describe the resulting store state.
- Implementation: [`Operations::commit_store_fallible()`](src/operations.rs) | Test: [jj-integration](tests/jj_integration.rs) t31

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
- Used for all issue-listing display (`list`, `list --tree`, `next`, `review`, `epic`, `flow`) and in creation/deletion/error messages (e.g., `Created bug 763`)
- The `show` command displays the full UUID
- Issue filenames keep the fixed 8-character short ID so they do not change as new issues are added (see [r[issue-filename-format]](#issue-filename-format))

Implementation: [`Operations::id_prefixes()`](src/operations.rs) and [`Operations::appropriate_length_id()`](src/operations.rs)

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

r[init-lost-index]
- `init [--force]` - Initialize pebbles repository

  `init` distinguishes the **empty store** from a **lost index** before
  writing an index:
  - **Empty store** - no `.pebbles/index.json` and no issue files under
    `.pebbles/issues/` (the directory may not exist yet). `init` proceeds:
    it creates the store and writes a fresh empty index.
  - **Lost index** - no `.pebbles/index.json`, but `.pebbles/issues/`
    contains issue files (e.g. after a bad `index.json` conflict
    resolution). A fresh empty index would silently orphan every issue, so
    `init` refuses with an error that names the rebuild-from-files path
    (`pebbles reindex`) and the number of orphaned files. `--force`
    overrides the refusal and writes the fresh empty index anyway; the
    orphan is then explicit, not silent.

  Example: [Quick Start](README.md#initialize)
  Implementation: [`Storage::issue_file_count()`](src/storage.rs), [`Operations::init()`](src/operations.rs) | Test: [jj-integration](tests/jj_integration.rs) t26, t27

r[new-issues-default-proposed]
- `new <title>` - Create new issue with optional `--issue-type` and `-d` for description  
  New issues start with status `proposed`. Passing `--approved` creates the issue with status `todo` instead.
  The title must be non-empty after trimming; empty or whitespace-only titles are an error.
  Example: [Quick Start](README.md#create-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#create-a-new-issue) | Review workflow: [review-workflow](tests/workflows/review-workflow.trycmd#review-proposed-issues)

r[type-templates]
- `new` additionally prompts for **type-specific template fields** when the
  create is interactive (stdin is a terminal). Each field asks for one piece
  of evidence the issue type needs; the answer is stored as metadata on the
  new issue (key = field key, value = answer; r[metadata-set-upsert]) and is
  shown by `show` and in exports like any other metadata - no extra display
  work. Field sets per issue type:

  - `bug`: `expected` (Expected behavior), `actual` (Actual behavior)
  - `ux_problem`: `before_screenshot` (Before screenshot (path or URL)),
    `after_screenshot` (After screenshot (path or URL))
  - `usecase`: `acceptance_criteria` (Acceptance criteria)
  - `design`, `validation`, `epic`, `milestone`: no template fields
    (behavior identical to `new` before templates existed)

  The prompts are **interactive-only**: when stdin is not a terminal (piped
  or scripted create, e.g. the trycmd harness), the fields are skipped
  silently - no prompt text is printed, nothing is read, and the create
  neither blocks nor waits. Every template field is optional: a blank answer
  (bare Enter, or EOF) is skipped and stores nothing for that key.
  Implementation: [`template_fields_for()`](src/operations.rs) (pure map) |
  Interactive prompting: [`prompt_template_fields()`](src/main.rs) |
  Test: [type-templates](tests/workflows/type-templates.trycmd#create-a-bug-from-piped-stdin-does-not-prompt-or-block)

r[guidance-tips]
After a successful `new`, pebbles may print optional guidance tips (hints,
never errors) so beginners and LLM users discover features contextually:
- If the new issue's type can be `part_of` an epic (every type but `epic`
  and `milestone`, r[part-of-epic-types]) and the project already has
  epics, print `Tip: Link to an epic with: pebbles link <id> part_of
  <epic-id>` followed by the existing epics (short id + title, one per
  line) so the user can pick a target.
- If the new issue has no relationships, print `Tip: Add dependencies
  with: pebbles link <id> required_for <other-id>`.
Both tips are gated on the project having at least one epic: with no epics
there is no pickable `part_of` target and the suggestions would be noise,
so creation output stays byte-identical for epic-less projects. The epic
tip is suppressed when the new issue is already part of an epic; the
dependency tip is suppressed when the issue already has relationships.
`<id>` is the issue's appropriate-length ID (r[appropriate-length-id]).
Tips appear after the issue detail block, one blank line before them.
Implementation: [`select_creation_tips()`](src/operations.rs) (pure) |
Rendering: [`print_creation_tips()`](src/display.rs) | Test:
[guidance](tests/workflows/guidance.trycmd#create-a-bug-now-that-an-epic-exists)

r[list-filter]
- `list` - List all issues with optional `--status`, `--issue-type`, `--search`,
  repeatable `--tag`, repeatable `--metadata`, and `--grep` filters  
  `--search <query>` fuzzy-matches issue titles (case-insensitive
  subsequence with word-boundary bonuses) and sorts by match quality, best
  first (as implemented in d5c; see `filter_by_title_query`).
  `--tag` keeps issues carrying every given tag (AND) and composes with the
  other filters and with `--search` and `--grep` (r[tag-filter-and]); an
  empty `--tag` value is an error.
  `--metadata <key>=<value>` keeps issues whose metadata carries every given
  pair (exact, case-sensitive match on both sides; r[metadata-filter-and]).
  Each value is split at the first `=`, so values may contain `=`; both sides
  are trimmed, and a pair without `=` or with an empty key is an error
  (r[metadata-filter-parse]). Composes with `--status`, `--issue-type`,
  `--search`, `--tag`, and `--grep`.
  `--grep <query>` keeps issues whose title or description contains every
  term of the query (case-insensitive substring; see r[grep-query-syntax]).
  Composes with `--status`, `--issue-type`, `--search`, `--tag`, and
  `--metadata`.
  Example: [Quick Start](README.md#list-and-filter) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#list-all-issues), [tag-management](tests/workflows/tag-management.trycmd#filter-by-tag), [metadata-management](tests/workflows/metadata-management.trycmd#filter-by-metadata), [full-text-search](tests/workflows/full-text-search.trycmd)

r[blocker-indicators]
Every `list` row (flat list, `next`, and `review`; the tree and epic-child
views keep their own layouts) ends with a dependency/blocker indicator
when the issue carries `required_for` relationships:
- `✓` (ready): the issue has no incomplete `required_for` dependencies
  (whether it has none at all or they are all `done`) and is itself not
  `done`.
- `⚠ (blocked by: <ids>)`: the issue depends, via `required_for`, on issues
  that are not `done`; `<ids>` are the minimal unambiguous ID prefixes of
  those open prerequisites (r[appropriate-length-id]).
- `→ (blocking: <ids>)`: other open issues depend on this one via
  `required_for`; informational only, so an issue that is blocked shows the
  blocked marker and never the blocking marker, and a `done` issue shows no
  marker at all.
`part_of` is containment, not a prerequisite, and never produces a marker.
Like `next` (see `get_unblocked_issues`), the status is computed over the
full issue set, so a prerequisite outside the current view still marks the
row. `scheduled_before` is a scheduling hint and likewise produces no
marker.
Implementation: [`build_blocker_status()`](src/operations.rs) (pure) |
Rendering: [`format_blocker_indicator()`](src/display.rs) (pure) | Test:
[core-workflow](tests/workflows/core-workflow.trycmd)

r[blocker-colors]
The relationship-health indicators (r[blocker-indicators]) and epic-membership
rows carry a fixed terminal palette, rendered through the crate's existing
`colored` abstraction in `display.rs`:

- `✓` green - issue has no incomplete dependencies, ready to work on.
- `⚠ (blocked by: ...)` yellow - issue is blocked by incomplete dependencies.
- `⛔ (circular)` red - issue participates in a dependency cycle; red takes
  precedence over the blocked/blocking markers.
- `→ (blocking: ...)` blue - issue is blocking other issues (informational).
- `[epic]` cyan; the child progress count `(done/total)` is colored by
  completion percentage: green at >=50%, yellow below 50% with at least one
  child done, red when nothing is done (0 done, including a childless
  `0/0`).
- Rows of non-epic issues that are part of no epic render gray when the
  view contains epics (an issue counts as "in an epic" when it carries a
  `part_of` relationship, matching `list --orphaned`); the health marker
  stays colored. In the tree view, the orphaned-issues section grays its
  rows under the same condition.

The markers themselves are the plain text from r[blocker-indicators] (68f);
color is an additive layer on top. Which issues can be circular:
`pebbles link` rejects edges that close a cycle and rejects self-edges
(r[graph-acyclic]), so a circular flag means the data
reached the store some other way (manual edit + reindex, import). The cycle
is computed over the dependency graph as `build_dependency_adjacency`
does - `required_for` plus `scheduled_before` edges; a self-referencing edge
is the minimal cycle. Implementation: [`build_circular_issues()`](src/operations.rs)
(pure) | Color selection: [`epic_count_color()`](src/display.rs),
[`epic_membership_gray()`](src/display.rs) (pure) | Verify: [display unit
tests](src/display.rs), [operations unit tests](src/operations.rs)

**No color when the terminal does not support it:** one decision per
invocation is made through `should_color()` in display.rs (SPEC
r[no-color-flag]) and applied as the `colored` crate's global override.
With no suppressor present, escape codes are emitted only when stdout is
a terminal. Piped output (including every trycmd fixture), `NO_COLOR=1`,
`CLICOLOR=0`, and `--no-color` all render the plain markers above with no
escape codes.

**Invalid relationships in `show`:** the detail view prints its outgoing
relationship lines with the target id cyan, but a self-referencing edge
(source == target) or a dangling target (the target issue does not exist in
the store) renders the id red (the self-edge is the minimal cycle of the
acyclicity invariant r[graph-acyclic]). Longer dependency cycles
are not visible to the detail view because it does not receive the
relationship graph; those are flagged by the list markers instead.
Implementation: [`show_relationship_health()`](src/display.rs) (pure) |
Verify: [display unit tests](src/display.rs)

r[no-color-flag]
Every colored command (list, show, epic, summary, flow, `next` tips, and the
post-create guidance) accepts `--no-color`. It is a root-level global flag
(clap `global = true`): `pebbles --no-color list` and, like other global
arguments, `pebbles list --no-color` both work. Precedence, strongest first:

1. `--no-color` - the explicit flag always suppresses color;
2. `NO_COLOR` set (any value) suppresses color;
3. `CLICOLOR=0` suppresses color;
4. otherwise color is emitted only when stdout is a terminal (piped output
   stays plain).

`CLICOLOR_FORCE=1` still forces color even when stdout is not a terminal,
but it ranks below `--no-color`, `NO_COLOR`, and `CLICOLOR=0` (the NO_COLOR
convention wins over forced color when both are set).

One decision per invocation: main.rs reads the flag and the environment,
calls the pure `should_color()` in display.rs, and applies the result as the
`colored` crate's global override, so every render path honors it uniformly.
Implementation: [`should_color()`](src/display.rs) (pure) | Verify: [display
unit tests](src/display.rs)

r[grep-query-syntax]
`pebbles list --grep <query>` runs a full-text search over issue titles and
descriptions with a small query syntax:
- Space-separated terms are ANDed: every term must match, as a
  case-insensitive substring, somewhere in the issue's title or description.
  An issue without a description has only its title to match.
- A double-quoted segment is a phrase: it must appear verbatim (internal
  spacing and word order preserved, still case-insensitive) as a substring.
- Matched issues keep the list's ordering (r[issue-ordering]: `updated_at`
  descending); `--grep` adds no relevance ranking of its own, unlike
  `--search` which sorts by fuzzy-match quality.

r[grep-query-parse]
A malformed `--grep` query is an error, not a silent fallback: an
unterminated quote (a `"` with no closing `"`) and an empty phrase (`""`,
which would match every issue) both fail the whole request atomically,
before any listing. An empty or whitespace-only query is valid but matches
nothing (r[continuity]): it is a distinct filter, not the unfiltered list,
mirroring `--search ""`.

Implementation: [`parse_text_query()`](src/operations.rs), [`filter_by_text_query()`](src/operations.rs) | Test: [operations unit tests](src/operations.rs), [full-text-search](tests/workflows/full-text-search.trycmd)

- `show <id>` - Show issue details  
  Example: [Quick Start](README.md#view-details) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#show-issue-details)

- `status <id> <status>` - Update issue status  
  Example: [Quick Start](README.md#update-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#update-issue-status)

r[approve-command]
- `approve <id>` - Approve a proposed issue  
  Moves an issue from `proposed` to `todo` and is equivalent to `status <id> todo` with a proposed-only guard: approving a non-proposed issue is an error.
  Test: [review-workflow](tests/workflows/review-workflow.trycmd#approve-a-proposed-issue) | Verification: [jj-integration](tests/jj_integration.rs) t10_approve

r[update-append]
- `update <id>` - Update issue title with `--title`, replace the description with `--description`, or append to it with `--append-description`  
  `--append-description` stores the text verbatim when there is no current description (or an empty one); a description already ending in a newline is extended in place; otherwise the text is appended after a newline. Appending the empty string is a no-op (no file write, no jj commit, "No changes: nothing to update"). `--description` and `--append-description` combine: the replacement applies first, then the append. Both flags keep the multi-ID/stdin input and per-ID failure summary of `update` (r[bulk-input], r[bulk-fail-soft]).
  Example: [Quick Start](README.md#update-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#update-issue-title), [update-append](tests/cmd/update-append.trycmd)

- `delete <id>` - Delete issue  
  Example: [Quick Start](README.md#delete-issues)

- `link <source> <relation> <target>` - Create relationship (`part_of`, `required_for`, `scheduled_before`)  
  Example: [Quick Start](README.md#link-issues) | Test: [core-workflow](tests/workflows/core-workflow.trycmd#link-issues-to-epic)

- `unlink <source> <relation> <target>` - Remove relationship  
  Example: [Quick Start](README.md#link-issues)

r[tag-add-dedupe]
- `tag add <id> <tag...>` - Add tags to an issue  
  Appends the given tags, deduplicating against the tags already present and
  within the input itself; when every requested tag is already present the
  command reports no changes and writes nothing. Tags are trimmed on entry;
  empty or whitespace-only tags are an error, and a single bad tag fails the
  whole request without adding anything.
  Test: [tag-management](tests/workflows/tag-management.trycmd#add-tags-to-an-issue)

- `tag remove <id> <tag...>` - Remove tags from an issue  
  Removes every occurrence of the given tags (exact, case-sensitive match);
  tags that are not present are ignored, and when none are present the
  command reports no changes and writes nothing. Empty tags are an error.
  Test: [tag-management](tests/workflows/tag-management.trycmd#remove-a-tag)

r[metadata-set-upsert]
- `metadata set <id> <key> <value>` - Set one metadata field on an issue  
  Upserts: a new key is inserted and an existing key's value is replaced in
  place. Keys and values are trimmed on entry; a key that is empty or
  whitespace-only is an error, and values may be empty (an empty value is a
  presence marker). When the key already carries exactly this value the
  command reports no changes and writes nothing. Exact, case-sensitive keys:
  `Priority` and `priority` are distinct fields.
  Test: [metadata-management](tests/workflows/metadata-management.trycmd#set-metadata-on-an-issue)

r[metadata-unset-absent]
- `metadata unset <id> <key>` - Remove one metadata field from an issue  
  Removes the key (exact, case-sensitive match); a key that is not present is
  a no-op that reports no changes and writes nothing. Empty keys are an
  error.
  Test: [metadata-management](tests/workflows/metadata-management.trycmd#unset-a-metadata-key)

r[metadata-get-sorted]
- `metadata get <id> [<key>]` - Show an issue's metadata  
  Without `<key>`, prints every pair as `key: value`, one per line, sorted by
  key (deterministic output). With `<key>`, prints just the value (script
  friendly); a key that is not set is an error. Empty keys are an error.
  Test: [metadata-management](tests/workflows/metadata-management.trycmd#get-all-metadata)

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

- `export <format> [-o <path>]` - Export all issues (`json`, `csv`, or
  `markdown`; stdout by default, `-o <path>` writes the file instead)
  See [Export and Import](#export-and-import) for the bundle format, CSV
  schema, Markdown shape, and deterministic ordering.

- `import <file>` - Import issues from a pebbles export JSON bundle
  (additive; validated fully before any write)  
  See [Export and Import](#export-and-import) for the validation rules,
  the dropped-relationship behavior, and the one-commit-per-import write
  path.

See [`Commands` enum](src/cli.rs#L11-L99) for complete argument specifications.

r[bulk-input]
r[bulk-fail-soft]
### Bulk Operations (multi-ID and stdin)

`status`, `delete`, `link`, `unlink`, and `update` accept multiple issue
IDs as arguments, or (when none are given) read them from stdin, one per
line, as printed by `pebbles list`, `next`, `review`, or `epic` (bare IDs
and full list rows are both accepted; see `parse_id_input_line`). This
enables Unix composition such as
`pebbles list --status todo | pebbles status in_progress`.

Input is resolved up front (r[bulk-input]): every ID is resolved to its
UUID before anything is written, duplicate IDs collapse to one operation,
and an unresolvable input (or unresolvable `link`/`unlink` target) aborts
the whole batch with an error before any change. There is no pre-flight
phase beyond resolution; each issue is applied as its own operation with
its own jj commit, in input order.

r[bulk-fail-soft]
On the happy path (all succeed) the output is the current one compact line
per changed issue (e.g. `✓ Updated 111 [bug] [TODO] Login bug`) and no
more; issues for which nothing needed to change print nothing, and the
exit code is 0. A single-issue batch prints exactly what the per-issue
command prints, including its error on failure.

Batches are fail-soft: a per-issue failure does not abort the rest of the
batch, and there is no rollback of issues already applied. Applied issues
keep their compact line on stdout; each failed ID is then reported on
stderr as `<operation> failed for <id>: <reason>` (the reason matches the
per-issue command's error), followed by a `Not applied: <ids>` line listing
what was NOT applied and a final `N/M succeeded` summary (`M` is the
resolved batch size, `N` the issues that did not fail). The command exits
non-zero when any issue failed. These semantics are identical for
argument-input and stdin-input batches.

Implementation: [`collect_id_inputs()`](src/main.rs), [`run_bulk_batch()`](src/main.rs)
Test: [jj-integration](tests/jj_integration.rs) t28-t32, [bulk operations](tests/cmd/bulk-operations.trycmd), [bulk failure summary](tests/cmd/bulk-failure.trycmd)

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

### Epic Progress Counts

r[epic-progress-counts]
Epics display child progress as a `(done/total)` ratio over `part_of` relationships:

- Without a view filter, one ratio is shown, covering all children of the epic.
- When a view filter is applied (`list --status`/`--issue-type`/`--orphaned`, `next`, `review`), two ratios are shown: the ratio over the filtered children first, then the ratio over the complete set — e.g. `(0/1, 1/3 total)`.
- The complete-set ratio is always computed over all issues, independent of the view filter; progress is a property of the epic, not of the view.

Implementation: [`Operations::epic_counts()`](src/operations.rs), [`build_epic_counts()`](src/models.rs) | Verify: [display tests](src/display.rs), [core-workflow](tests/workflows/core-workflow.trycmd)

### Interactive TUI

r[tui]
**Browser (`pebbles tui`):**
- Two-pane layout: left = foldable tree (epics with child pebbles newest-first, epic `(done/total)` progress, then orphaned pebbles newest-first); right = detail pane for the highlighted issue mirroring `show` (ID, type, status, description, tags, relationships, incoming references, updated timestamp); multi-line descriptions render as separate lines and long lines word-wrap.
- Tree and filter lists sort by status (proposed, todo, in progress, blocked, done), then by type (bug, design, others), newest first within the same status+type: users read from the top.
- Navigation: `j`/`k` or Up/Down move; Right unfolds (or moves to the first child), Left folds (or moves to the parent); Enter or `f` toggles the fold; Tab/BackTab switch which pane owns the keyboard (r[tui-focus]); `/` jumps to the search line (r[tui-search]); `e` opens the inline edit pane for title/description/type/status (r[tui-edit]); `?` toggles an opaque help overlay; `p` refreshes the projects from disk on demand (r[tui-refresh]); `q` quits and restores the terminal (leaves alt-screen, shows cursor).
- Refresh-on-demand (`p` from the tree, r[tui-refresh]): the TUI loads each project's issue set once per invocation and re-reads it only after its own mutations - no polling and no watcher, nothing is re-read between keypresses. `p` re-runs project discovery with the startup config (home/repos, the config's extra dirs, and the current directory when it is a project) and reloads every project from disk, so external state changes become visible: another pebbles process, an agent session, a manual edit, a jj checkout, or a new project appearing in the registry. Newly discovered projects are added and vanished ones dropped, keeping the registry order. The selection re-settles exactly like after a mutation (r[tui-review]): the highlight stays on the same item while it is still visible.
- Every draw clears the full frame and the (alternate) screen is cleared once at startup: cells left behind by previous terminal contents never show through unpainted areas (r[tui-background]).
- Display prefixes follow r[appropriate-length-id] semantics (minimal unambiguous prefixes where IDs are shown).

r[tui-review]
**Review filter (key `r` toggles it, `Esc`/`r` clears it):**
- A quick status filter on the main two-pane UI, not a separate mode: the left pane lists only proposed issues and its title shows the filter; the right pane keeps showing the selected issue's detail.
- `a` approves the selected proposed issue (r[approve-command]); `d` then `y` closes it by deleting it (r[no-broken-links] guards apply and surface as error messages). Outside the filter, `a`/`d` only hint at the filter.
- After any mutation the issue set reloads and the selection re-settles in place: the highlight stays on the same item while it is still visible, otherwise it moves by exactly one position - to the item that followed it, or to the new last visible item when the removed one was last - so `a`/`d` keep acting on what is listed instead of teleporting to the top.
- Open question: whether closing a proposal should become a dedicated rejected status instead of delete (tracked in the review-queue issue).

r[tui-background]
**Clean background:** the (alternate) screen is cleared once at startup (escape-sequence clear, so it never waits on a cursor-position query that no terminal answers) and every rendered frame clears the whole frame first. Unpainted cells therefore always come out blank instead of showing whatever the terminal displayed before the TUI started.

Implementation: [`tui::run()`](src/tui.rs) | Test: [unit + TestBackend tests](src/tui.rs), [tui e2e](tests/tui_e2e.rs)

r[tui-focus]
**Pane focus (key Tab/BackTab cycles, `/` jumps to search):**
- The browser has three panes that can own the keyboard: the tree (move/select, `r` filter, `a`/`d` mutations, `p` refresh, r[tui-refresh]), the search line (query editing, r[tui-search]), and the detail pane (`j`/`k` scroll its content). Tab and BackTab cycle focus; the focused pane's border is highlighted, and the search line additionally shows the `search: <query>` prompt while focused.
- Enter in the search line accepts the query and returns focus to the tree; Esc clears the query and returns to the tree; Tab leaves the search open while focus moves on, so browsing and the query can coexist.
- `/` jumps to the search line from the tree or the detail pane. `q` quits from the tree and the detail pane.

Implementation: [`tui::run()`](src/tui.rs) | Test: [unit + TestBackend tests](src/tui.rs), [tui e2e](tests/tui_e2e.rs)

r[tui-edit]
**Inline edit pane (key `e` with an issue selected in the detail pane):**
- The right pane lists Title, Description, Type, and Status with the live values; the left tree stays visible. `j`/`k` or Up/Down move between fields (Tab/BackTab step too), and the selected field is marked `> `.
- While editing any field, every row except the active one shows the issue's LIVE value; the active typed field is rendered by a ratatui-textarea widget that shows the text, the cursor and the selection.
- Typed fields (Title and Description): `Enter` starts editing (`arm`); the textarea is pre-filled with the current value and the cursor starts at the end. Editing supports arrow-key movement (`Left`/`Right`/`Up`/`Down`), `Home`/`End`, `Ctrl+Left`/`Ctrl+Right` word moves, `Backspace`/`Delete` at the cursor, and typed characters inserted at the cursor (multi-byte safe). Shift+arrows select text; `Ctrl+c` copies, `Ctrl+x` cuts and `Ctrl+y` pastes through the widget's internal yank buffer (multi-line yanks paste as multiple lines); `Ctrl+u`/`Ctrl+r` undo/redo. The pane's own keys stay unchanged: `Enter` (Title: applies; Description: inserts a newline) and `Tab` (applies either field); the applied field runs one operation through the operations layer: `update` for title/description (keeping the non-empty-title validation) (r[update]).
- Cycled fields (Type and Status): `Left`/`Right` cycle the pending value in navigation, in the display order `bug, ux_problem, usecase, design, validation, epic, milestone` and `proposed, todo, in_progress, done, blocked`, wrapping both ways. `Enter` applies the cycle immediately and stays in the pane (commit-and-stay): it does not arm a second editing step, and the highlight stays on the field with the pending value re-synced to the applied one. Each apply runs one operation and therefore creates its own jj commit: `convert` for type, `update_status` for status (r[convert-command], r[update-status]); the issue set reloads, the tree re-selects the edited issue, and the message line reports the change.
- `Esc` cancels without writing: during a field edit it discards the pending text and returns to navigation; in navigation it closes the pane. No operation runs until a field is applied (with `Enter`, or `Tab` for a multi-line Description). Uncommitted cycles are dropped when the pane closes. No-change applies report "No changes" and write nothing.
- Failures (empty title, convert guards such as converting an epic with children) surface in the message line with the pane still open. `q` quits while navigating the pane, not during text entry.

r[tui-search]
**Quick search (key `/` opens the search line; search applies live as you type):**
- Enter accepts the query and leaves the search line; the tree title then shows the active query. Esc (in the search line or with an active search) clears it. Backspace edits the query while typing, and Tab moves focus elsewhere without closing the search (r[tui-focus]).
- The query is a list of whitespace-separated terms; every term must match (AND). While active, the left pane lists matching issues flat per project (like the review filter), and projects with no matches are hidden. An empty query shows the full tree.
- A bare term is an exact-substring filter against the issue's short id, title, and description: case-insensitive, literal (no regex or wildcards). Terms are split on spaces only, so `pane edit` matches a title of "Edit pane" (each term matches somewhere in the issue, in any order).
- `s:` filters by status, `t:` by issue type, using one-letter codes or full names: status `p`/proposed, `t`/todo, `i`/in_progress (also `inprogress`, `in-progress`), `b`/blocked, `d`/done; type `bug`, `ux` (also `ux_problem`), `uc` (also `usecase`), `des` (also `design`), `val` (also `validation`), `epic`, `mile` (also `milestone`). Prefixes are case-insensitive (`S:T` equals `s:t`).
- A leading `!` negates a term everywhere: `s:!d` anything but done, `t:!bug` anything but bugs, `!pane` an issue whose fields do not contain "pane".
- An unparseable spec (`s:x`, `t:9`) or a bare `!` matches no issue.

Implementation: [`tui::run()`](src/tui.rs) | Test: [tui e2e](tests/tui_e2e.rs), [unit + TestBackend tests](src/tui.rs)

### Projects

r[project-registry]
**Multi-project registry (`pebbles tui`):**
- A project is a directory containing a `.pebbles/` directory.
- Discovery: scan one level of `~/repos/`, plus additional directories from the config file `~/.config/pebbles/projects.json` (`{"extra_dirs": ["/path/1", "/path/2"]}`; an absent or malformed config contributes nothing). The current directory is always included when it contains `.pebbles/`, so single-project usage works unchanged from any directory.
- Ordering: projects sort by the modification time of their `.pebbles/index.json`, most recent first; a project without an index sorts last; ties order by directory name.
- The TUI tree shows projects as the top level, above epics. A project node carries its total issue count and unfolds into that project's epic/orphan structure. The detail pane for a project node shows its path, per-status issue counts, and the last index modification time.
- Mutations (approve, close) route to the issue's own project: each project keeps its own jj repo and per-operation commits (or `.no_jj` skips them in tests).
- With a filter active (status review filter or a search query), only projects that have matching issues are listed.
- Implementation: [`projects::discover()`](src/projects.rs) | Test: [tui e2e](tests/tui_e2e.rs), [unit tests](src/projects.rs)

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
- Import: "Import {N} issue(s) from {file}" (one commit per import)

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

The `metadata` field is a `string -> string` map that can store arbitrary
key-value pairs. This allows for:
- Type-specific fields without schema changes
- User-defined attributes
- Tool-specific data

Manage metadata through the CLI: `pebbles metadata set <id> <key> <value>`
upserts one field (r[metadata-set-upsert]), `pebbles metadata unset <id>
<key>` removes one field (a no-op when absent; r[metadata-unset-absent]), and
`pebbles metadata get <id> [<key>]` prints all pairs or a single value
(r[metadata-get-sorted]). `pebbles list --metadata <key>=<value>` filters by
metadata: the flag is repeatable with AND semantics (an issue must carry
every given pair, matched exactly and case-sensitively on both sides) and
composes with `--status`, `--issue-type`, `--search`, and `--tag`
(r[metadata-filter-and], r[metadata-filter-parse]). Keys and values are
trimmed on entry; keys must be non-empty, values may be empty. Stored as a
`BTreeMap` so on-disk serialization is deterministic, keys sorted, keeping
issue files stable for diffs and merges (r[metadata-file-order]). Use the CLI
for all supported updates; edit JSON manually only if the CLI is failing —
manual edits bypass pebbles' validation rules and jj commits.

### Tags

The `tags` field is a `Vec<String>` for categorization. Manage tags through
the CLI: `pebbles tag add <id> <tag...>` appends tags (deduplicating against
existing tags and within the input; a no-op change reports no changes) and
`pebbles tag remove <id> <tag...>` removes them. `pebbles list --tag <tag>`
filters by tag: the flag is repeatable with AND semantics (an issue must
carry every given tag) and composes with `--status`, `--issue-type`, and
`--search`. Tags are trimmed on entry and matched exactly
(case-sensitive); empty tags are an error. Use the CLI for all supported
updates; edit JSON manually only if the CLI is failing.

## Export and Import

r[export-order]
Every export output is deterministic: JSON and CSV list issues in ascending
id order (string order of the UUIDs), and the Markdown document lists
epics, children, and orphans by title. Input order from storage never
reaches the output, so exports are canonical and diff-friendly.

### JSON bundle format

r[export-bundle]
`pebbles export json` writes the full issue set as a versioned JSON
envelope, and `pebbles import` accepts exactly this format:

```json
{
  "format": "pebbles-export",
  "version": 1,
  "issues": [ /* one Issue object per issue, as in the issue file schema */ ]
}
```

- `format` must be exactly `"pebbles-export"` and `version` exactly `1`;
  anything else, a missing or mis-typed field, or non-object/non-array
  shapes are rejected up front. Unknown envelope keys are ignored.
- Each `issues` element is one [`Issue`](src/models.rs) object preserving
  id, title, description, status, issue_type, tags, relationships,
  metadata, and both timestamps. The bundle is the only import format;
  there is no generic "import from another tracker".
- Issues are listed in ascending id order (string order of the UUIDs) so
  exports are canonical and diff-friendly. Export is a pure function of
  the stored issue set plus this ordering rule (r[export-order]); imports
  write issues in bundle order.
- Implementation: [`export.rs`](src/export.rs) | Tests: [export unit tests](src/export.rs), [export-import workflow](tests/workflows/export-import.trycmd)

r[import-semantics]
`pebbles import <file>` validates the bundle completely before writing
anything:

1. The file parses as the envelope above; an issue element that fails the
   same schema checks as a loaded issue file (a `serde` deserialization
   into `Issue`) fails the whole import naming its 1-based position.
   Elements with keys outside the schema import with a warning naming the
   keys, which are dropped on write - mirroring the store's load path.
2. Duplicate ids inside the bundle are rejected.
3. An id that already exists in the store is rejected: import is
   additive and never overwrites. Rejections write nothing.
4. A relationship whose target is neither in the bundle nor in the store
   is **dropped with a warning** (the store's no-broken-links invariant,
   r[impl no-broken-links], holds after the import). Relationships whose
   target exists in the store are kept.
5. Every accepted issue is written through the existing
   `Storage::save_issue` path and the store change is committed to jj in
   ONE commit: `Import N issue(s) from <file>` (r[jj-commit-complete-path-set]).

Implementation: [`parse_export_bundle`](src/export.rs), [`validate_import`](src/export.rs), [`drop_missing_relationships`](src/export.rs), [`Operations::import_bundle()`](src/operations.rs) | Tests: [export unit tests](src/export.rs), [export-import workflow](tests/workflows/export-import.trycmd)

### CSV schema

r[csv-schema]
`pebbles export csv` writes a flat, deterministic table:

- One header row `id,title,description,status,issue_type,tags,relationships,metadata`,
  then one row per issue sorted by id (r[export-order]).
- Comma delimiter, RFC-4180 escaping: every field is double-quoted and
  embedded double quotes are doubled (`""`). Records end with LF
  (RFC-4180 specifies CRLF; LF is accepted by every CSV consumer and
  keeps exports diff-friendly).
- `description` is empty when the issue has none; `status` and
  `issue_type` use their canonical on-disk spellings; `tags`,
  `relationships`, and `metadata` are JSON-encoded so no information is
  lost (e.g. `["a","b"]`, `[{"relation_type":"part_of","target_id":"..."}]`,
  `{"priority":"high"}`). The CSV is a lossless, machine-readable
  projection of the bundle.

### Markdown document

r[export-markdown]
`pebbles export markdown` writes a readable document for humans
(best-effort, not lossless): one `## Epic: <title>` section per epic with
its part_of children as `- [ ] <id> [<type>] [<status>] <title>` rows
(`[x]` for done issues), then an `## Orphans` section with every non-epic
issue that is not a child of any epic in the export. Epics, children, and
orphans sort by title (r[export-order]). Tags, metadata, descriptions, and
non-epic relationships are not rendered; use `export json` or
`export csv` for the complete data.

## Future Considerations

Features mentioned in historical documentation but not yet implemented:
- Templates per issue type

These features are tracked as pebbles issues in the repository.
