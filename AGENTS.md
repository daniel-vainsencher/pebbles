# AI Agents Guide for Pebbles

This guide is for AI coding assistants working on the pebbles codebase.

## Essential Reading

**All AI agents must read and follow [CONTRIBUTING.md](CONTRIBUTING.md).**

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
./target/debug/pebbles list
cargo test
cargo clippy
cargo fmt
```

That's it. Everything else is in CONTRIBUTING.md.
