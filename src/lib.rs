//! Library target for pebbles.
//!
//! The crate is primarily a CLI binary (`src/main.rs`). This lib target exists
//! so that `cargo test` compiles and runs the doctests on the documented
//! modules instead of failing with "no library targets found".
//!
//! The bin-only crate previously never compiled doctests: `cargo test` only
//! runs them for library targets, and none existed.

pub mod export;
pub mod models;
pub mod operations;
pub mod storage;

// Internal module required to compile the public ones. It stays private to the
// lib so its own doc comments are not surfaced as lib API.
mod jj;
