use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

pub struct JJ {
    repo_path: std::path::PathBuf,
}

impl JJ {
    pub fn new(repo_path: &Path) -> Result<Self> {
        Ok(Self {
            repo_path: repo_path.to_path_buf(),
        })
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

    pub fn commit(&self, message: &str) -> Result<()> {
        let output = Command::new("jj")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to execute jj commit")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to commit changes: {}", stderr);
        }

        Ok(())
    }

    pub fn describe(&self, message: &str) -> Result<()> {
        let output = Command::new("jj")
            .arg("describe")
            .arg("-m")
            .arg(message)
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to execute jj describe")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to describe changes: {}", stderr);
        }

        Ok(())
    }

    pub fn new_change(&self) -> Result<()> {
        let output = Command::new("jj")
            .arg("new")
            .current_dir(&self.repo_path)
            .output()
            .context("Failed to execute jj new")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create new change: {}", stderr);
        }

        Ok(())
    }
}
