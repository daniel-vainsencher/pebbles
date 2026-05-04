use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct JJ {
    repo_path: PathBuf,
    pebbles_path: PathBuf,
}

impl JJ {
    pub fn new(repo_path: &Path) -> Result<Self> {
        // If PEBBLES_DB_PATH is set, use it as the pebbles directory
        // and its parent as the repo path
        let (pebbles_path, effective_repo_path) =
            if let Ok(custom_path) = env::var("PEBBLES_DB_PATH") {
                let pebbles = PathBuf::from(&custom_path);
                let repo = if pebbles.is_relative() {
                    repo_path
                        .join(&pebbles)
                        .parent()
                        .unwrap_or(repo_path)
                        .to_path_buf()
                } else {
                    pebbles.parent().unwrap_or(&pebbles).to_path_buf()
                };
                (pebbles, repo)
            } else {
                (repo_path.join(".pebbles"), repo_path.to_path_buf())
            };

        Ok(Self {
            repo_path: effective_repo_path,
            pebbles_path,
        })
    }

    // r[impl no-jj-skip]
    fn should_skip_jj(&self) -> bool {
        // Skip jj operations if .no_jj marker file exists
        self.pebbles_path.join(".no_jj").exists()
    }

    pub fn is_repo(&self) -> bool {
        self.repo_path.join(".jj").exists()
    }

    pub fn init(&self) -> Result<()> {
        if self.is_repo() {
            return Ok(());
        }

        let output = Command::new("jj")
            .arg("git")
            .arg("init")
            .arg("--colocate")
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to execute jj init")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to initialize jj repo: {}", stderr);
        }

        Ok(())
    }

    /// Run `jj commit -m <message> <paths...>`.
    ///
    /// All `paths` are passed to jj so the resulting commit contains exactly
    /// the changes touching those paths. If any change pebbles made is
    /// omitted from `paths`, that change will remain in the working copy
    /// instead of being committed — so callers must pass the *complete* set
    /// of paths affected by the operation, including any deleted files,
    /// renamed files (both old and new), and `.pebbles/index.json` when the
    /// index has changed.
    ///
    /// `paths` must be non-empty.
    // r[impl jj-commit-complete-path-set]
    pub fn commit(&self, message: &str, paths: &[&Path]) -> Result<()> {
        // Skip jj operations if .no_jj marker exists
        if self.should_skip_jj() {
            return Ok(());
        }

        if paths.is_empty() {
            bail!("JJ::commit called with no paths");
        }

        let mut cmd = Command::new("jj");
        cmd.arg("commit").arg("-m").arg(message);
        for path in paths {
            cmd.arg(path);
        }

        let output = cmd
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to execute jj commit")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to commit changes: {}", stderr);
        }

        Ok(())
    }
}
