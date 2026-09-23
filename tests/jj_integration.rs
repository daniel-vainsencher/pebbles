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

    /// Run `pebbles <args>` with `stdin` on its stdin, returning the raw
    /// stdout/stderr and exit status. Used by the fail-soft batch tests,
    /// which need to assert both the applied lines (stdout) and the failure
    /// summary (stderr) of a single invocation.
    fn pebbles_io(&self, args: &[&str], stdin: &str) -> std::process::Output {
        use std::io::Write;
        let mut child = Command::new(PEBBLES_BIN)
            .args(args)
            .current_dir(&self.root)
            .env("PEBBLES_DB_PATH", ".pebbles")
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("FORCE_COLOR")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn pebbles");
        child
            .stdin
            .take()
            .expect("stdin pipe")
            .write_all(stdin.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait for pebbles")
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

/// T11: `pebbles unlink` commit message must name the target issue, not the
/// literal `<target>` placeholder.
// r[verify no-duplicate-relationships]
#[test]
fn t11_unlink_commit_message_names_target() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Source", "-t", "bug"]);
    let src_id = repo.find_id(|_| true);

    repo.pebbles(&["new", "Target", "-t", "bug"]);
    let tgt_id = repo.find_id(|e| e.id != src_id);

    repo.pebbles(&["link", &src_id, "required_for", &tgt_id]);
    repo.pebbles(&["unlink", &src_id, "required_for", &tgt_id]);

    let desc = repo.last_commit_description();
    assert!(
        desc.contains("Remove relationship: Source required_for Target"),
        "unlink commit message should name the target, got: {:?}",
        desc
    );
}

/// T12: linking an issue to itself must be rejected with no commit.
#[test]
fn t12_self_link_rejected() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Lonely", "-t", "bug"]);
    let id = repo.find_id(|_| true);
    let file = repo.find_file_name(|_| true);

    let before_count = repo.commit_count();
    let err = repo.pebbles_fail(&["link", &id, "required_for", &id]);
    assert!(
        err.to_lowercase().contains("itself"),
        "error should mention self-linking, got: {:?}",
        err
    );

    // No commit was made and the issue file has no relationships.
    let content = repo.read_issue_file(&file);
    assert!(
        !content.contains("required_for"),
        "self-edge must not be stored, got: {:?}",
        content
    );
    assert_eq!(
        repo.commit_count(),
        before_count,
        "rejected link must not create a commit"
    );
    repo.assert_clean_wc("after rejected self-link");
}

/// T13: short ID input is case-insensitive (SPEC r[short-id-input]).
#[test]
fn t13_short_id_input_case_insensitive() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);

    let upper = id[..4].to_uppercase();
    repo.pebbles(&["show", &upper]);
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

/// T16: output messages use the appropriate-length ID, never the raw argument
/// or the full UUID (SPEC r[appropriate-length-id]).
#[test]
fn t16_messages_use_appropriate_length_id() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let out = repo.pebbles(&["new", "Foo bug", "-t", "bug"]);
    let id = repo.find_id(|_| true);

    // The creation message already uses the canonical short id; capture it.
    let short = out
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Created bug ")
        .expect("creation message")
        .to_string();
    assert!(short.len() >= 3 && id.starts_with(&short));

    // Approving a non-proposed issue errors with the short id, not the UUID.
    repo.pebbles(&["approve", &id]); // proposed -> todo
    let err = repo.pebbles_fail(&["approve", &id]);
    assert!(
        err.contains(&short) && !err.contains(&id),
        "approve error should use short id {:?}, got: {:?}",
        short,
        err
    );

    // Deleting via the full UUID echoes the short id, not the raw argument.
    let out = repo.pebbles(&["delete", &id]);
    assert_eq!(out.trim(), format!("✓ Deleted issue {}", short));
}

/// T19: a link that closes a directed dependency cycle is rejected
/// (r[graph-acyclic]).
// r[verify graph-acyclic]
#[test]
fn t19_link_creating_cycle_rejected() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "A", "-t", "bug", "--approved"]);
    let a_id = repo.find_id(|_| true);
    repo.pebbles(&["new", "B", "-t", "bug", "--approved"]);
    let b_id = repo.find_id(|e| e.id != a_id);

    repo.pebbles(&["link", &a_id, "required_for", &b_id]);
    let before = repo.commit_count();

    let err = repo.pebbles_fail(&["link", &b_id, "required_for", &a_id]);
    assert!(
        err.to_lowercase().contains("cycle"),
        "error should mention the cycle, got: {:?}",
        err
    );
    assert_eq!(
        repo.commit_count(),
        before,
        "rejected link must not create a commit"
    );
    repo.assert_clean_wc("after rejected cycle link");
}

/// T20: longer cycles are rejected too, for both dependency relation types
/// (r[graph-acyclic]).
#[test]
fn t20_longer_cycle_rejected() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    for title in ["A", "B", "C"] {
        repo.pebbles(&["new", title, "-t", "bug", "--approved"]);
    }
    let ids: Vec<String> = repo.read_index().iter().map(|e| e.id.clone()).collect();
    let (a, b, c) = (&ids[0], &ids[1], &ids[2]);

    repo.pebbles(&["link", a, "required_for", b]);
    repo.pebbles(&["link", b, "scheduled_before", c]);
    let before = repo.commit_count();

    // C -> A closes A -> B -> C -> A.
    let err = repo.pebbles_fail(&["link", c, "required_for", a]);
    assert!(
        err.to_lowercase().contains("cycle"),
        "error should mention the cycle, got: {:?}",
        err
    );

    // A path of length 2 also counts: B -> C exists, so C -> B is a cycle.
    let err = repo.pebbles_fail(&["link", c, "scheduled_before", b]);
    assert!(
        err.to_lowercase().contains("cycle"),
        "error should mention the cycle, got: {:?}",
        err
    );
    assert_eq!(repo.commit_count(), before);
    repo.assert_clean_wc("after rejected longer cycles");
}

/// T22: deleting a referenced issue is guarded by r[no-broken-links]:
/// it fails naming the broken links unless --delete-links-too removes them.
// r[verify no-broken-links]
#[test]
fn t22_delete_guard_and_delete_links_too() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let out = repo.pebbles(&["new", "Source", "-t", "bug", "--approved"]);
    let src_id = repo.find_id(|_| true);
    let src_file = repo.find_file_name(|_| true);
    let src_short = out
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Created bug ")
        .expect("creation message");

    repo.pebbles(&["new", "Target", "-t", "bug", "--approved"]);
    let tgt_id = repo.find_id(|e| e.id != src_id);
    let tgt_file = repo.find_file_name(|e| e.id != src_id);

    repo.pebbles(&["link", &src_id, "required_for", &tgt_id]);

    // Guarded delete fails and names the linking issue.
    let err = repo.pebbles_fail(&["delete", &tgt_id]);
    assert!(
        err.contains("--delete-links-too") && err.contains(src_short),
        "error should name the broken link and the escape hatch, got: {:?}",
        err
    );

    // --delete-links-too removes the referencing relationship first.
    repo.pebbles(&["delete", &tgt_id, "--delete-links-too"]);
    repo.assert_clean_wc("after guarded delete");
    repo.assert_last_commit_paths(
        "guarded delete",
        &p(&[&issue_path(&src_file), &issue_path(&tgt_file), INDEX_PATH]),
    );

    let content = repo.read_issue_file(&src_file);
    assert!(
        !content.contains("required_for"),
        "referencing relationship must be gone, got: {:?}",
        content
    );
    assert_eq!(repo.read_index().len(), 1, "target is gone");
}

/// T24: the epic error uses the appropriate-length id (r[appropriate-length-id]).
#[test]
fn t24_epic_error_uses_short_id() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let out = repo.pebbles(&["new", "Not an epic", "-t", "bug", "--approved"]);
    let id = repo.find_id(|_| true);
    let short = out
        .lines()
        .next()
        .unwrap()
        .strip_prefix("Created bug ")
        .expect("creation message");

    let err = repo.pebbles_fail(&["epic", &id]);
    assert!(
        err.contains(short) && !err.contains(&id),
        "epic error should use the appropriate-length id {:?}, got: {:?}",
        short,
        err
    );
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

/// T25: `pebbles new` recreates a missing `.pebbles/issues/` directory
/// (r[issues-dir-created-on-demand]). Checking out a commit that predates
/// the issue files removes the directory together with its last file,
/// because jj tracks files, not directories.
// r[verify issues-dir-created-on-demand]
#[test]
fn t25_new_self_heals_missing_issues_dir() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Seed issue", "-t", "bug", "--approved"]);
    let seed_id = repo.find_id(|_| true);

    // Move the working copy to the commit before the seed issue: jj removes
    // the issue file and, with it, the now-empty issues directory.
    repo.jj(&["new", "@--"]);
    assert!(
        !repo.root.join(".pebbles/issues").exists(),
        "setup: issues dir should be absent after checking out the earlier commit"
    );

    // The mutation must self-heal: recreate the directory and write the file.
    repo.pebbles(&["new", "Second issue", "-t", "bug", "--approved"]);
    let second_file = repo.find_file_name(|e| e.id != seed_id);

    repo.assert_clean_wc("after self-healing new");
    repo.assert_last_commit_paths(
        "self-healing new",
        &p(&[&issue_path(&second_file), INDEX_PATH]),
    );
    repo.assert_no_paths_outside_pebbles("self-healing new");

    // The directory exists again and holds the new issue file; the index
    // only knows the new issue, since the checkout reverted the seed entry.
    assert!(
        repo.root
            .join(".pebbles/issues")
            .join(&second_file)
            .exists(),
        "recreated issues dir must contain the new issue file"
    );
    assert_eq!(
        repo.read_index().len(),
        1,
        "checkout reverted the seed index entry"
    );
}

/// T26: init must not silently orphan a store whose index.json is missing
/// while issue files remain (the lost-index case). Without --force it
/// refuses and points at the rebuild-from-files path (`pebbles reindex`);
/// `--force` writes a fresh empty index anyway, orphaning the issues by
/// explicit choice (SPEC r[init-lost-index]).
// r[verify init-lost-index]
#[test]
fn t26_init_refuses_lost_index_without_force() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Alpha", "-t", "bug", "--approved"]);
    repo.pebbles(&["new", "Beta", "-t", "bug", "--approved"]);
    assert_eq!(repo.read_index().len(), 2, "two issues indexed");

    // Simulate a lost index (e.g. a bad conflict resolution of index.json):
    // the issue files remain, index.json is gone.
    std::fs::remove_file(repo.root.join(".pebbles/index.json")).expect("remove index");

    // Without --force, init must refuse, write nothing, and point at the
    // rebuild-from-files path.
    let err = repo.pebbles_fail(&["init"]);
    assert!(
        err.contains("index.json") && err.contains("orphan"),
        "refusal should explain the lost-index state, got: {:?}",
        err
    );
    assert!(
        err.contains("reindex"),
        "refusal should point at the rebuild-from-files path, got: {:?}",
        err
    );
    assert!(
        err.contains("2 issue file"),
        "refusal should name the orphan count, got: {:?}",
        err
    );
    assert!(
        !repo.root.join(".pebbles/index.json").exists(),
        "refused init must not write an index"
    );
    assert_eq!(
        issue_file_count(&repo.root),
        2,
        "refused init must not touch the issue files"
    );

    // The store is still in the lost-index state: list stays unusable.
    let err = repo.pebbles_fail(&["list"]);
    assert!(err.contains("not initialized"), "got: {:?}", err);

    // --force writes the fresh empty index (explicit orphan).
    repo.pebbles(&["init", "--force"]);
    repo.assert_clean_wc("after forced init");
    repo.assert_last_commit_paths("forced init", &p(&[INDEX_PATH]));
    repo.assert_no_paths_outside_pebbles("forced init");
    assert_eq!(repo.read_index().len(), 0, "fresh empty index");
    assert_eq!(
        issue_file_count(&repo.root),
        2,
        "the issue files are orphaned, not deleted"
    );
    let out = repo.pebbles(&["list"]);
    assert!(
        out.contains("No issues found."),
        "orphaned issues are invisible, got: {:?}",
        out
    );
}

/// T27: init on an empty store (no index, issues directory empty or absent)
/// still succeeds: the empty-store and lost-index cases are distinguishable
/// (SPEC r[init-lost-index]).
#[test]
fn t27_init_empty_store_succeeds() {
    // Fresh repo: nothing exists yet.
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.assert_clean_wc("after fresh init");
    repo.assert_last_commit_paths("fresh init", &p(&[INDEX_PATH]));

    // Re-init is refused: an existing index means already initialized.
    let err = repo.pebbles_fail(&["init"]);
    assert!(err.contains("already initialized"), "got: {:?}", err);

    // A store with an issues directory but no files is still an empty store.
    let repo = TestRepo::new();
    std::fs::create_dir_all(repo.root.join(".pebbles/issues")).expect("issues dir");
    repo.pebbles(&["init"]);
    repo.assert_clean_wc("after init with empty issues dir");
}

/// T28: a bulk `status` makes one commit per issue, each touching exactly
/// that issue's file, and leaves the working copy clean.
#[test]
fn t28_bulk_status_commits_per_issue() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Alpha", "-t", "bug", "--approved"]);
    let a_id = repo.find_id(|_| true);
    let a_file = repo.find_file_name(|_| true);
    repo.pebbles(&["new", "Beta", "-t", "bug", "--approved"]);
    let b_id = repo.find_id(|e| e.id != a_id);
    let b_file = repo.find_file_name(|e| e.id != a_id);

    // Both IDs in one invocation: two commits, one per issue, in input order.
    repo.pebbles(&["status", &a_id, &b_id, "in_progress"]);
    repo.assert_clean_wc("after bulk status");

    // The last commit is the second issue processed (Beta).
    repo.assert_last_commit_paths("bulk status: Beta", &p(&[&issue_path(&b_file)]));
    // The commit before it is the first issue processed (Alpha).
    let alpha_commit =
        parse_summary_paths(&repo.jj(&["diff", "--from", "@---", "--to", "@--", "--summary"]));
    assert_eq!(
        alpha_commit,
        p(&[&issue_path(&a_file)]),
        "bulk status must commit one path set per issue"
    );
    repo.assert_no_paths_outside_pebbles("bulk status");
}

/// T29: a bulk `link` (multiple sources, one target) makes one commit per
/// source, each touching exactly that source's file.
#[test]
fn t29_bulk_link_commits_per_source() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Alpha", "-t", "bug", "--approved"]);
    let a_id = repo.find_id(|_| true);
    let a_file = repo.find_file_name(|_| true);
    repo.pebbles(&["new", "Beta", "-t", "bug", "--approved"]);
    let b_id = repo.find_id(|e| e.id != a_id);
    let b_file = repo.find_file_name(|e| e.id != a_id);
    repo.pebbles(&["new", "Epic", "-t", "epic", "--approved"]);
    let epic_id = repo.find_id(|e| e.id != a_id && e.id != b_id);

    repo.pebbles(&["link", &a_id, &b_id, "part_of", &epic_id]);
    repo.assert_clean_wc("after bulk link");
    repo.assert_last_commit_paths("bulk link: Beta", &p(&[&issue_path(&b_file)]));
    let alpha_commit =
        parse_summary_paths(&repo.jj(&["diff", "--from", "@---", "--to", "@--", "--summary"]));
    assert_eq!(alpha_commit, p(&[&issue_path(&a_file)]));
    repo.assert_no_paths_outside_pebbles("bulk link");
}

/// T30: `pebbles reindex` rebuilds a conflicted (unparsable) index from the
/// issue files and verifies with a load; the rebuilt index is left in the
/// working copy, uncommitted (the command never runs jj - a conflicted
/// index is exactly the repair context where committing makes no sense).
#[test]
fn t30_reindex_repairs_conflicted_index() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Alpha bug", "-t", "bug"]);
    repo.pebbles(&["new", "Beta idea", "-t", "design"]);
    let before_commits = repo.commit_count();
    assert_eq!(repo.read_index().len(), 2, "two issues indexed");

    // Simulate a conflicted index file: every command fails to parse it and
    // no repair path exists other than reindex.
    std::fs::write(
        repo.root.join(".pebbles/index.json"),
        "conflicted <<<<<<< garbage",
    )
    .expect("corrupt index");
    let err = repo.pebbles_fail(&["list"]);
    assert!(err.contains("Failed to parse index file"), "got: {:?}", err);

    // A concurrent agent left a hand-written issue file too: reindex must
    // adopt it (the files are the truth).
    let new_file = "bug-55555555-hand-written-issue.json";
    std::fs::write(
        repo.root.join(".pebbles/issues").join(&new_file),
        r#"{
           "id": "55555555-5555-5555-5555-555555555555",
           "title": "Hand written issue",
           "description": null,
           "status": "proposed",
           "issue_type": "bug",
           "tags": [],
           "relationships": [],
           "metadata": {},
           "created_at": "2026-01-10T12:01:00Z",
           "updated_at": "2026-01-10T12:01:00Z"
       }
       "#,
    )
    .expect("write hand-written issue");

    let out = repo.pebbles(&["reindex"]);
    assert!(out.contains("Rebuilt index: 3"), "got: {:?}", out);

    // Verified with a load: the index parses and every issue still lists,
    // including the hand-written one.
    assert_eq!(repo.read_index().len(), 3, "index rebuilt from all files");
    let listed = repo.pebbles(&["list"]);
    assert!(listed.contains("Hand written issue"), "got: {:?}", listed);

    // reindex never commits: the working copy holds exactly the new issue
    // file and the repaired index, and no new commit appeared.
    let diff = repo.jj(&["diff", "--summary"]);
    let paths = parse_summary_paths(&diff);
    assert_eq!(
        &paths,
        &p(&[&issue_path(&new_file), INDEX_PATH]),
        "reindex leaves only the index change: {:?}",
        paths
    );
    assert_eq!(
        repo.commit_count(),
        before_commits,
        "reindex must not commit (repair context)"
    );
    assert!(
        repo.jj(&["log", "--no-pager", "-r", "@-", "-T", "description"])
            .contains("Beta idea"),
        "last commit still the last mutation"
    );
}

/// T31: a store without a jj repository still writes the store and exits 0,
/// with the uncommitted-change fact surfaced as a warning: the exit code
/// describes the resulting store state, not jj's (r[store-without-jj]).
#[test]
fn t31_store_without_jj_repo_writes_and_exits_0() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    repo.pebbles(&["new", "Alpha", "-t", "bug", "--approved"]);

    // Drop the jj repo while the store stays (e.g. someone deleted .jj).
    remove_dir_all(&repo.root.join(".jj")).expect("remove .jj");

    let output = Command::new(PEBBLES_BIN)
        .args(&["new", "Beta", "-t", "bug", "--approved"])
        .current_dir(&repo.root)
        .env("PEBBLES_DB_PATH", ".pebbles")
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .output()
        .expect("run pebbles");
    assert!(
        output.status.success(),
        "store write must succeed without a jj repo: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No jj repository found"),
        "succeeding without committing must be loud: {:?}",
        stderr
    );

    // The resulting store state is intact: the new issue is written,
    // indexed, and listed.
    assert_eq!(repo.read_index().len(), 2, "new issue is indexed");
    let listed = String::from_utf8_lossy(&output.stdout);
    assert!(listed.contains("Created bug"), "got: {:?}", listed);
}

/// Recursively delete a directory tree (jj tools are not guaranteed present,
/// so tests remove `.jj` with the std library).
fn remove_dir_all(path: &std::path::Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)?.flatten() {
        if entry.path().is_dir() {
            remove_dir_all(&entry.path())?;
        } else {
            std::fs::remove_file(&entry.path())?;
        }
    }
    std::fs::remove_dir(path)
}

/// Common setup for the fail-soft bulk link tests: Alpha required_for Beta,
/// so linking Beta required_for Alpha is a cycle rejection. Returns the
/// (id, short id, file name) triples for Alpha, Beta, and Gamma.
struct ThreeBugs {
    a_id: String,
    a_short: String,
    a_file: String,
    b_id: String,
    b_short: String,
    b_file: String,
    c_id: String,
    c_short: String,
    c_file: String,
}

/// Create three approved bugs; return their ids, display short ids (from the
/// `Created bug <short>` line), and file names.
fn three_bugs(repo: &TestRepo) -> ThreeBugs {
    let a_out = repo.pebbles(&["new", "Alpha", "-t", "bug", "--approved"]);
    let a_id = repo.find_id(|_| true);
    let a_file = repo.find_file_name(|_| true);
    let b_out = repo.pebbles(&["new", "Beta", "-t", "bug", "--approved"]);
    let b_id = repo.find_id(|e| e.id != a_id);
    let b_file = repo.find_file_name(|e| e.id != a_id);
    let c_out = repo.pebbles(&["new", "Gamma", "-t", "bug", "--approved"]);
    let c_id = repo.find_id(|e| e.id != a_id && e.id != b_id);
    let c_file = repo.find_file_name(|e| e.id != a_id && e.id != b_id);

    let short = |out: &str| {
        out.lines()
            .next()
            .unwrap()
            .strip_prefix("Created bug ")
            .expect("creation message")
            .to_string()
    };
    ThreeBugs {
        a_id,
        a_short: short(&a_out),
        a_file,
        b_id,
        b_short: short(&b_out),
        b_file,
        c_id,
        c_short: short(&c_out),
        c_file,
    }
}

/// T34: a bulk `link` is fail-soft: when one source would close a
/// dependency cycle, the batch still applies the other sources, prints one
/// compact line per applied issue, reports each failed ID with its reason,
/// lists what was NOT applied, prints the `N/M succeeded` summary, and
/// exits non-zero. Only the applied source gets a jj commit.
#[test]
fn t32_bulk_link_partial_failure_applies_rest_and_reports() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let bugs = three_bugs(&repo);

    // Alpha required_for Beta; Beta required_for Alpha would close a cycle.
    repo.pebbles(&["link", &bugs.a_id, "required_for", &bugs.b_id]);
    let before = repo.commit_count();

    // Gamma -> Alpha applies; Beta -> Alpha fails per-issue.
    let out = repo.pebbles_io(
        &["link", &bugs.c_id, &bugs.b_id, "required_for", &bugs.a_id],
        "",
    );
    assert!(
        !out.status.success(),
        "a batch with any failed ID must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    // The applied source keeps its compact line on stdout.
    assert!(
        stdout.contains(&format!(
            "✓ Added relationship: {} required_for {}",
            bugs.c_short, bugs.a_short
        )),
        "applied source should keep its compact line, stdout: {:?}",
        stdout
    );
    assert!(
        !stdout.contains("Beta"),
        "no success line for the failed source, stdout: {:?}",
        stdout
    );

    // The failure summary names operation, ID, and reason; lists what was
    // not applied; and closes with the N/M succeeded count, all on stderr.
    assert!(
        stderr.contains(&format!(
            "Link failed for {}: Cannot add relationship Beta required_for Alpha: it would create a dependency cycle",
            bugs.b_short
        )),
        "failure line missing, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains(&format!("Not applied: {}", bugs.b_short)),
        "not-applied list missing, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains("1/2 succeeded"),
        "summary line missing, stderr: {:?}",
        stderr
    );

    // Exactly one jj commit: Gamma's link. Beta's rejected link must not
    // touch anything, and the working copy must be clean.
    assert_eq!(repo.commit_count(), before + 1);
    repo.assert_clean_wc("after partial-failure bulk link");
    repo.assert_last_commit_paths(
        "partial-failure bulk link: Gamma",
        &p(&[&issue_path(&bugs.c_file)]),
    );
    repo.assert_no_paths_outside_pebbles("partial-failure bulk link");
}

/// T33: the fail-soft batch semantics apply to stdin-driven bulk input too
/// (`pebbles link required_for <target>` with sources on stdin): applied
/// sources commit, failed sources are reported per-ID, and the exit code is
/// non-zero.
#[test]
fn t33_bulk_link_stdin_partial_failure() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let bugs = three_bugs(&repo);

    repo.pebbles(&["link", &bugs.a_id, "required_for", &bugs.b_id]);
    let before = repo.commit_count();

    // Sources read from stdin, Gamma first (applies), Beta second (cycle).
    let stdin_input = format!(
        "{}
{}
",
        bugs.c_id, bugs.b_id
    );
    let out = repo.pebbles_io(&["link", "required_for", &bugs.a_id], &stdin_input);
    assert!(
        !out.status.success(),
        "a stdin batch with any failed ID must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        stdout.contains(&format!(
            "✓ Added relationship: {} required_for {}",
            bugs.c_short, bugs.a_short
        )),
        "applied source should keep its compact line, stdout: {:?}",
        stdout
    );
    assert!(
        stderr.contains(&format!("Link failed for {}: ", bugs.b_short)) && stderr.contains("cycle"),
        "failure line missing, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains(&format!("Not applied: {}", bugs.b_short)),
        "not-applied list missing, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains("1/2 succeeded"),
        "summary line missing, stderr: {:?}",
        stderr
    );

    assert_eq!(repo.commit_count(), before + 1);
    repo.assert_clean_wc("after stdin partial-failure bulk link");
    repo.assert_last_commit_paths(
        "stdin partial-failure bulk link: Gamma",
        &p(&[&issue_path(&bugs.c_file)]),
    );
    repo.assert_no_paths_outside_pebbles("stdin partial-failure bulk link");
}

/// T34: a bulk `delete` is fail-soft too — deleting the second of two
/// issues fails when it is referenced by a third issue (r[no-broken-links]);
/// the first is still deleted (one jj commit), the failed ID is reported
/// with its reason, and the command exits non-zero.
#[test]
fn t34_bulk_delete_partial_failure_reports_guarded_id() {
    let repo = TestRepo::new();
    repo.pebbles(&["init"]);
    let bugs = three_bugs(&repo);

    // Gamma references Beta, so deleting Beta is guarded (r[no-broken-links]).
    repo.pebbles(&["link", &bugs.c_id, "required_for", &bugs.b_id]);
    let before = repo.commit_count();

    // Alpha deletes; Beta fails because Gamma still references it.
    let out = repo.pebbles_io(&["delete", &bugs.a_id, &bugs.b_id], "");
    assert!(
        !out.status.success(),
        "a batch with any failed ID must exit non-zero"
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        stdout.contains(&format!("✓ Deleted issue {}", bugs.a_short)),
        "applied source should keep its compact line, stdout: {:?}",
        stdout
    );
    assert!(
        !stdout.contains("Beta"),
        "no success line for the failed ID, stdout: {:?}",
        stdout
    );
    assert!(
        stderr.contains(&format!("Delete failed for {}: ", bugs.b_short))
            && stderr.contains("--delete-links-too"),
        "failure line missing its reason, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains(&format!("Not applied: {}", bugs.b_short)),
        "not-applied list missing, stderr: {:?}",
        stderr
    );
    assert!(
        stderr.contains("1/2 succeeded"),
        "summary line missing, stderr: {:?}",
        stderr
    );

    // Exactly one jj commit: Alpha's delete (issue file + index).
    assert_eq!(repo.commit_count(), before + 1);
    repo.assert_clean_wc("after partial-failure bulk delete");
    repo.assert_last_commit_paths(
        "partial-failure bulk delete: Alpha",
        &p(&[&issue_path(&bugs.a_file), INDEX_PATH]),
    );
    repo.assert_no_paths_outside_pebbles("partial-failure bulk delete");
}
/// Count `*.json` files under `<root>/.pebbles/issues/`.
fn issue_file_count(root: &std::path::Path) -> usize {
    let dir = root.join(".pebbles/issues");
    std::fs::read_dir(&dir)
        .expect("read issues dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .count()
}
