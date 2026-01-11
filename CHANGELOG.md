# Changelog

All notable changes to Pebbles will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-01-06

### Added
- **Issue Types**: Support for 7 different issue types
  - `bug` - Software defects
  - `ux_problem` - User experience issues
  - `usecase` - User stories and requirements
  - `design` - Software design problems
  - `validation` - Validation features
  - `epic` - Container for grouping related issues
  - `milestone` - For prioritization and ordering
- **Relationship Types**: Three types of relationships between issues
  - `part_of` - Issue is part of an epic (with validation)
  - `required_for` - Dependency relationship
  - `scheduled_before` - Ordering constraint
- **Meaningful File Naming**: Issues now stored as `type-shortid-slug.json`
  - Examples: `bug-a1b2c3d4-login-broken.json`, `epic-e5f6g7h8-auth-feature.json`
  - Automatic file renaming when title changes
  - Better jj diffs with readable filenames
- **Link/Unlink Commands**: Manage relationships via CLI
  - `pebbles link <source> <relation-type> <target>`
  - `pebbles unlink <source> <relation-type> <target>`
- **Blocked Status**: New status option for blocked issues
- **Type-based Filtering**: Filter issues by type
  - `pebbles list --issue-type bug`
- **Enhanced Display**: Show issue types, relationships, and metadata
- **Relationship Validation**: Prevent invalid relationships (e.g., epic can't be part of epic)
- **Metadata Field**: Extensible key-value storage for custom fields
- **Type-specific Colors**: Each issue type has its own color in listings

### Changed
- **Index Structure**: Now stores `IssueMetadata` (id + filename) instead of just UUIDs
- **Issue Model**: Added `issue_type`, `relationships`, and `metadata` fields
- **File Storage**: Files now use meaningful names instead of UUIDs
- **Display Format**: List view now shows both type and status badges
- **Commit Messages**: Now include issue type in jj commit descriptions

### Removed
- **Dependencies Field**: Replaced by `relationships` with explicit relationship types

### Dependencies Added
- `slug` 0.1 - For generating URL-safe slugs from titles
- `fuzzy-matcher` 0.3 - For future fuzzy search feature
- `ratatui` 0.29 - For future TUI feature
- `crossterm` 0.29 - Terminal handling for TUI

### Breaking Changes
- Old v0.1.0 repositories are not directly compatible
- Issue files renamed from `<uuid>.json` to `<type>-<shortid>-<slug>.json`
- Index format changed (migration required)
- `dependencies` field removed (use `relationships` instead)

## [0.1.0] - 2026-01-02

### Added
- Initial release
- Basic CRUD operations for issues
- Status tracking (todo, in_progress, done)
- Jujutsu integration
- Short ID support (8 characters)
- Status filtering
- Colored terminal output
- JSON-based storage in `.pebbles/` directory
- Simple CLI with clap
- Tag support
- Dependency tracking (simple UUID list)

### Architecture
- Clean separation: models, storage, operations, jj, cli, display
- Individual JSON files per issue
- Index file tracking all issues
- Automatic jj commits for all operations

[0.2.0]: https://github.com/yourusername/pebbles/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/yourusername/pebbles/releases/tag/v0.1.0
