# Guide to working on the Pebbles project

Is in [CONTRIBUTING.md](CONTRIBUTING.md). Before making any changes, make sure to read and internalize also:

- **[README.md](README.md)** - What pebbles does and how users interact with it
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Design philosophy, system architecture, extension points
- **[SPEC.md](SPEC.md)** - Data model, file formats, and behavior specifications

## Versioning

Releases follow Semantic Versioning (https://semver.org): MAJOR.MINOR.PATCH.

- MAJOR: backward-incompatible changes (breaking CLI, issue file format, or data model changes).
- MINOR: backward-compatible new features (new commands, TUI modes, issue types, flags).
- PATCH: backward-compatible bug fixes and documentation-only changes.

Bump the version in Cargo.toml (and Cargo.lock) in the release commit, describe it as
`chore: release vMAJOR.MINOR.PATCH`, tag the release commit (`jj tag set vMAJOR.MINOR.PATCH -r @-`),
and push the release bookmark through sync (jjs workflow, see ~/repos/jjs/README.md).
