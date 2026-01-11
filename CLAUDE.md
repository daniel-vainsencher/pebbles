# Claude Code Guide for Pebbles

This guide is for Claude Code working on the pebbles codebase.

## Essential Reading

**Read [CONTRIBUTING.md](CONTRIBUTING.md) first.**

CONTRIBUTING.md contains everything you need:
- Development workflow (build, test, install)
- Jujutsu version control workflow
- Code style and quality standards
- Documentation rules
- Testing requirements

## Other Documentation

- **[README.md](README.md)** - What pebbles does and how users interact with it
- **[ARCHITECTURE.md](ARCHITECTURE.md)** - Design philosophy, system architecture, extension points
- **[SPEC.md](SPEC.md)** - Data model, file formats, and behavior specifications

## Quick Start

```bash
# Read CONTRIBUTING.md first, then:
cargo build
./target/debug/pebbles <command>
cargo test
cargo clippy
```

That's it. Everything else is in CONTRIBUTING.md.
