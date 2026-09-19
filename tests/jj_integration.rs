//! Integration tests for pebbles' jj integration semantics.
//!
//! Each test runs the pebbles binary against a fresh tempfile::TempDir as a
//! brand-new colocated jj repo. After every mutating operation, three
//! invariants are checked:
//!
//! 1. The working copy is clean (`jj diff --summary` is empty).
//! 2. Nothing outside `.pebbles/` has been committed (checked over the whole
//!    test's history at the end).
//! 3. The most recent commit (`@-`) touches exactly the expected set of paths.
//!
//! Tests use the binary via `CARGO_BIN_EXE_pebbles` and learn issue filenames
//! by reading `.pebbles/index.json` after each command — no library access
//! into pebbles internals.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

const PEBBLES_BIN: &str = env!("CARGO_BIN_EXE_pebbles");

// --- Test harness -----------------------------------------------------------

struct TestRepo {
    _tempdir: tempfile::TempDir,
    root: PathBuf,
}

impl TestRepo {
    /// Create a fresh tempdir. Caller must invoke `pebbles_init` to make it a
    /// jj repo + initialize pebbles.
    fn new() -> Self {
        let tempdir = tempfile::Builder::new()
            .prefix("pebbles-jj-test-")
            .tempdir()
            .expect("create tempdir");
        let root = tempdir.path().to_path_buf();
        TestRepo {
            _tempdir: tempdir,
            root,
        }
    }

    /// Run `pebbles <args>` in the tempdir with PEBBLES_DB_PATH=.pebbles.
    /// Returns stdout on success, panics on failure with stderr.
    fn pebbles(&self, args: &[&str]) -> String {
        let output = Command::new(PEBBLES_BIN)
            .args(args)
            .current_dir(&self.root)
            .env("PEBBLES_DB_PATH", ".pebbles")
            // Avoid colored output bleeding into anything we parse.
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("FORCE_COLOR")
            .output()
            .expect("run pebbles");
        if !output.status.success() {
            panic!(
                "pebbles {:?} failed (status {}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
                args,
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        String::from_utf8(output.stdout).expect("utf8 stdout")
    }

    /// Run `jj <args>` in the tempdir, capturing stdout.
    fn jj(&self, args: &[&str]) -> String {
        let output = Command::new("jj")
            .args(args)
            .current_dir(&self.root)
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("FORCE_COLOR")
            .output()
            .expect("run jj");
        if !output.status.success() {
            panic!(
                "jj {:?} failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
                args,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        String::from_utf8(output.stdout).expect("utf8 stdout")
    }

    /// Read `.pebbles/index.json` and return its issue entries.
    fn read_index(&self) -> Vec<IndexEntry> {
        let path = self.root.join(".pebbles/index.json");
        let s = std::fs::read_to_string(&path).expect("read index");
        let idx: Index = serde_json::from_str(&s).expect("parse index");
        idx.issues
    }

    /// Look up the on-disk filename of the single issue matching `predicate`.
    /// Panics if zero or more than one match.
    fn find_file_name(&self, predicate: impl Fn(&IndexEntry) -> bool) -> String {
        let entries: Vec<_> = self.read_index().into_iter().filter(predicate).collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one matching index entry, got {}",
            entries.len()
        );
        entries.into_iter().next().unwrap().file_name
    }

    /// Look up the full UUID of the single issue matching `predicate`.
    fn find_id(&self, predicate: impl Fn(&IndexEntry) -> bool) -> String {
        let entries: Vec<_> = self.read_index().into_iter().filter(predicate).collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one matching index entry, got {}",
            entries.len()
        );
        entries.into_iter().next().unwrap().id
    }

    // --- Invariant assertions -------------------------------------------

    /// Invariant 1: working copy is clean (no uncommitted changes).
    fn assert_clean_wc(&self, context: &str) {
        let out = self.jj(&["diff", "--summary"]);
        assert!(
            out.trim().is_empty(),
            "[{}] working copy not clean:\n{}",
            context,
            out
        );
    }

    /// Invariant 3: the most recent commit (@-) touches exactly `expected`.
    // r[verify jj-commit-complete-path-set]
    fn assert_last_commit_paths(&self, context: &str, expected: &BTreeSet<String>) {
        let out = self.jj(&["diff", "--from", "@--", "--to", "@-", "--summary"]);
        let actual = parse_summary_paths(&out);
        assert_eq!(
            &actual, expected,
            "[{}] last commit paths mismatch.\nactual:   {:?}\nexpected: {:?}\nraw summary:\n{}",
            context, actual, expected, out
        );
    }

    /// Invariant 2: across the whole history (all commits since root), every
    /// affected path is under `.pebbles/`.
    fn assert_no_paths_outside_pebbles(&self, context: &str) {
        let out = self.jj(&["diff", "--from", "root()", "--to", "@-", "--summary"]);
        let paths = parse_summary_paths(&out);
        let stray: Vec<_> = paths
            .iter()
            .filter(|p| !p.starts_with(".pebbles/") && p.as_str() != ".pebbles")
            .collect();
        assert!(
            stray.is_empty(),
            "[{}] commits touched paths outside .pebbles/: {:?}\nraw:\n{}",
            context,
            stray,
            out
        );
    }

    /// Run `pebbles <args>` expecting failure. Returns stderr.
    fn pebbles_fail(&self, args: &[&str]) -> String {
        let output = Command::new(PEBBLES_BIN)
            .args(args)
            .current_dir(&self.root)
            .env("PEBBLES_DB_PATH", ".pebbles")
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("FORCE_COLOR")
            .output()
            .expect("run pebbles");
        assert!(
            !output.status.success(),
            "pebbles {:?} was expected to fail but succeeded:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8_lossy(&output.stderr).to_string()
    }

    /// Description of the most recent non-working-copy commit (`@-`).
    fn last_commit_description(&self) -> String {
        self.jj(&["log", "--no-pager", "-r", "@-", "-T", "description"])
    }

    /// Number of commits reachable from `@-` (i.e. committed changes).
    fn commit_count(&self) -> usize {
        let out = self.jj(&[
            "log",
            "--no-pager",
            "-r",
            "::@-",
            "-T",
            "commit_id ++ \"\n\"",
        ]);
        out.lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Read an issue file's raw JSON content.
    fn read_issue_file(&self, file_name: &str) -> String {
        std::fs::read_to_string(self.root.join(".pebbles/issues").join(file_name))
            .expect("read issue file")
    }
}

// --- Index parsing ----------------------------------------------------------

#[derive(Deserialize)]
struct Index {
    issues: Vec<IndexEntry>,
}

#[derive(Deserialize, Clone)]
struct IndexEntry {
    id: String,
    file_name: String,
}

// --- jj diff --summary parser ----------------------------------------------

/// Parse `jj diff --summary` output into the set of affected paths.
///
/// jj summary lines look like:
///   `A path`         (added)
///   `M path`         (modified)
///   `D path`         (deleted)
///   `R prefix{old => new}`   (rename — contributes both prefix+old and prefix+new)
///   `C prefix{old => new}`   (copy — same shape as rename, contributes both)
///
/// The `prefix` may be empty, in which case the line looks like `R {old => new}`.
fn parse_summary_paths(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in s.lines() {
        let line = line.trim_end_matches('\n');
        if line.is_empty() {
            continue;
        }
        // Lines start with a single status character followed by a space.
        let (kind, rest) = line.split_once(' ').unwrap_or(("", line));
        match kind {
            "A" | "M" | "D" => {
                out.insert(rest.to_string());
            }
            "R" | "C" => {
                // rest is either "{old => new}" or "prefix/{old => new}".
                let (prefix, inner) = match rest.find('{') {
                    Some(i) => (&rest[..i], &rest[i..]),
                    None => panic!("malformed rename line: {:?}", line),
                };
                // inner now starts with '{' and ends with '}'.
                let inner = inner
                    .strip_prefix('{')
                    .and_then(|s| s.strip_suffix('}'))
                    .unwrap_or_else(|| panic!("malformed rename line: {:?}", line));
                let (old, new) = inner
                    .split_once(" => ")
                    .unwrap_or_else(|| panic!("malformed rename inner: {:?}", inner));
                out.insert(format!("{}{}", prefix, old));
                out.insert(format!("{}{}", prefix, new));
            }
            other => panic!("unknown summary line kind {:?} in {:?}", other, line),
        }
    }
    out
}

#[test]
fn parse_summary_basic() {
    let s = "A foo/bar.txt\nM x.txt\nD y.txt\n";
    let got = parse_summary_paths(s);
    assert_eq!(
        got,
        ["foo/bar.txt", "x.txt", "y.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn parse_summary_renames() {
    // Same-directory rename:
    let got = parse_summary_paths("R sub/{old.txt => new.txt}\n");
    assert_eq!(
        got,
        ["sub/old.txt", "sub/new.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );

    // Cross-directory rename:
    let got = parse_summary_paths("R {a/f.txt => b/g.txt}\n");
    assert_eq!(
        got,
        ["a/f.txt", "b/g.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>()
    );
}

// --- Helpers for building expected path sets -------------------------------

fn p(parts: &[&str]) -> BTreeSet<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn issue_path(file_name: &str) -> String {
    format!(".pebbles/issues/{}", file_name)
}

const INDEX_PATH: &str = ".pebbles/index.json";

// --- Tests ------------------------------------------------------------------

/// T1: `pebbles init` should commit just `.pebbles/index.json`.
#[test]
fn t1_init() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.assert_clean_wc("after init");
    repo.assert_last_commit_paths("init", &p(&[INDEX_PATH]));
    repo.assert_no_paths_outside_pebbles("init");
}

/// T2: `pebbles new` should commit the new issue file AND `index.json`.
#[test]
fn t2_create() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    repo.assert_clean_wc("after new");

    let file = repo.find_file_name(|_| true);
    repo.assert_last_commit_paths("new", &p(&[&issue_path(&file), INDEX_PATH]));
    repo.assert_no_paths_outside_pebbles("new");
}

/// T3: `pebbles status` should commit just the issue file (no index churn).
#[test]
fn t3_status_change() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);

    repo.pebbles(&["status", &id, "in_progress"]);
    repo.assert_clean_wc("after status");
    repo.assert_last_commit_paths("status", &p(&[&issue_path(&file)]));
    repo.assert_no_paths_outside_pebbles("status");
}

/// T4: `pebbles update --description` should commit just the issue file.
#[test]
fn t4_update_description() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);

    repo.pebbles(&["update", &id, "-d", "new description"]);
    repo.assert_clean_wc("after update --description");
    repo.assert_last_commit_paths("update -d", &p(&[&issue_path(&file)]));
    repo.assert_no_paths_outside_pebbles("update -d");
}

/// T5: `pebbles update --title` (rename) should commit new file, old file
/// (deletion), and `index.json`.
// r[verify rename-commits-all-paths]
#[test]
fn t5_rename_via_title() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let old_file = repo.find_file_name(|_| true);

    repo.pebbles(&["update", &id, "-t", "Renamed bug"]);
    repo.assert_clean_wc("after rename");

    let new_file = repo.find_file_name(|_| true);
    assert_ne!(old_file, new_file, "filename should have changed");

    repo.assert_last_commit_paths(
        "rename",
        &p(&[&issue_path(&old_file), &issue_path(&new_file), INDEX_PATH]),
    );
    repo.assert_no_paths_outside_pebbles("rename");
}

/// T6: `pebbles link` should commit just the source issue file.
#[test]
fn t6_link() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Source", "-t", "bug"]);
    let src_id = repo.find_id(|_| true);
    let src_file = repo.find_file_name(|_| true);

    repo.pebbles(&["new", "Target", "-t", "bug"]);
    let tgt_id = repo.find_id(|e| e.id != src_id);

    repo.pebbles(&["link", &src_id, "required_for", &tgt_id]);
    repo.assert_clean_wc("after link");
    repo.assert_last_commit_paths("link", &p(&[&issue_path(&src_file)]));
    repo.assert_no_paths_outside_pebbles("link");
}

/// T7: `pebbles unlink` should commit just the source issue file.
#[test]
fn t7_unlink() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Source", "-t", "bug"]);
    let src_id = repo.find_id(|_| true);
    let src_file = repo.find_file_name(|_| true);

    repo.pebbles(&["new", "Target", "-t", "bug"]);
    let tgt_id = repo.find_id(|e| e.id != src_id);

    repo.pebbles(&["link", &src_id, "required_for", &tgt_id]);
    repo.pebbles(&["unlink", &src_id, "required_for", &tgt_id]);
    repo.assert_clean_wc("after unlink");
    repo.assert_last_commit_paths("unlink", &p(&[&issue_path(&src_file)]));
    repo.assert_no_paths_outside_pebbles("unlink");
}

/// T8: `pebbles convert` (type rename) should commit new file, old file
/// (deletion), and `index.json`.
// r[verify rename-commits-all-paths]
#[test]
fn t8_convert() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let old_file = repo.find_file_name(|_| true);

    repo.pebbles(&["convert", &id, "usecase"]);
    repo.assert_clean_wc("after convert");

    let new_file = repo.find_file_name(|_| true);
    assert_ne!(old_file, new_file, "filename should have changed");

    repo.assert_last_commit_paths(
        "convert",
        &p(&[&issue_path(&old_file), &issue_path(&new_file), INDEX_PATH]),
    );
    repo.assert_no_paths_outside_pebbles("convert");
}

/// T9: `pebbles delete` should commit the deleted issue file AND `index.json`.
#[test]
fn t9_delete() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);

    repo.pebbles(&["delete", &id]);
    repo.assert_clean_wc("after delete");
    repo.assert_last_commit_paths("delete", &p(&[&issue_path(&file), INDEX_PATH]));
    repo.assert_no_paths_outside_pebbles("delete");
}

/// T10: `pebbles approve` should commit just the source issue file (no index
/// churn, no rename).
// r[verify approve-command]
#[test]
fn t10_approve() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);

    repo.pebbles(&["approve", &id]);
    repo.assert_clean_wc("after approve");
    repo.assert_last_commit_paths("approve", &p(&[&issue_path(&file)]));
    repo.assert_no_paths_outside_pebbles("approve");
}

/// T14: empty and whitespace-only titles are rejected (create and update).
#[test]
fn t14_empty_title_rejected() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);

    let err = repo.pebbles_fail(&["new", ""]);
    assert!(
        err.contains("title"),
        "error should mention title: {:?}",
        err
    );

    let err = repo.pebbles_fail(&["new", "   "]);
    assert!(
        err.contains("title"),
        "error should mention title: {:?}",
        err
    );

    // A valid issue exists for the update check.
    repo.pebbles(&["new", "Real issue", "-t", "bug", "--approved"]);
    let id = repo.find_id(|_| true);

    let err = repo.pebbles_fail(&["update", &id, "--title", "  "]);
    assert!(
        err.contains("title"),
        "error should mention title: {:?}",
        err
    );

    // Nothing was created by the rejected commands.
    let count = repo.read_index().len();
    assert_eq!(count, 1, "only the valid issue should exist");
}

/// T15: no-op `update` and `status` make no commit and leave the file intact.
#[test]
fn t15_noop_operations_make_no_commit() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug", "--approved"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);
    let before_count = repo.commit_count();
    let before_content = repo.read_issue_file(&file);

    // update with no flags: nothing to change.
    repo.pebbles(&["update", &id]);
    assert_eq!(
        repo.commit_count(),
        before_count,
        "no-op update must not create a commit"
    );

    // status to the current status (todo): nothing to change.
    repo.pebbles(&["status", &id, "todo"]);
    assert_eq!(
        repo.commit_count(),
        before_count,
        "no-op status must not create a commit"
    );

    assert_eq!(
        repo.read_issue_file(&file),
        before_content,
        "no-op operations must not touch the issue file (updated_at)"
    );
    repo.assert_clean_wc("after no-op operations");
}

/// T18: converting an issue to its current type is a no-op with a warning.
#[test]
fn t18_convert_same_type_warns() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug", "--approved"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);
    let before_count = repo.commit_count();
    let before_content = repo.read_issue_file(&file);

    let out = repo.pebbles(&["convert", &id, "bug"]);
    assert!(
        out.contains("already a bug"),
        "should warn on same-type conversion, got: {:?}",
        out
    );

    assert_eq!(
        repo.commit_count(),
        before_count,
        "no-op convert must not create a commit"
    );
    assert_eq!(
        repo.read_issue_file(&file),
        before_content,
        "no-op convert must not touch the issue file"
    );
    repo.assert_clean_wc("after no-op convert");
}

/// T17: `flow --format` rejects values other than text/svg.
#[test]
fn t17_flow_rejects_unknown_format() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug", "--approved"]);

    let err = repo.pebbles_fail(&["flow", "--format", "jsonl"]);
    assert!(
        err.contains("text") && err.contains("svg"),
        "error should name the valid formats, got: {:?}",
        err
    );
}
