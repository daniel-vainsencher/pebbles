use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::{Index, Issue};

pub struct Storage {
    pebbles_dir: PathBuf,
    issues_dir: PathBuf,
    index_path: PathBuf,
}

impl Storage {
    pub fn new(repo_path: &Path) -> Result<Self> {
        // Allow overriding pebbles directory via environment variable (for testing)
        let pebbles_dir = if let Ok(custom_path) = env::var("PEBBLES_DB_PATH") {
            PathBuf::from(custom_path)
        } else {
            repo_path.join(".pebbles")
        };
        let issues_dir = pebbles_dir.join("issues");
        let index_path = pebbles_dir.join("index.json");

        Ok(Self {
            pebbles_dir,
            issues_dir,
            index_path,
        })
    }

    pub fn init(&self) -> Result<()> {
        fs::create_dir_all(&self.issues_dir)
            .context("Failed to create .pebbles/issues directory")?;

        if !self.index_path.exists() {
            let index = Index::new();
            self.save_index(&index)?;
        }

        Ok(())
    }

    pub fn exists(&self) -> bool {
        self.pebbles_dir.exists() && self.index_path.exists()
    }

    pub fn issue_path(&self, file_name: &str) -> PathBuf {
        self.issues_dir.join(file_name)
    }

    pub fn index_path(&self) -> &PathBuf {
        &self.index_path
    }

    pub fn load_index(&self) -> Result<Index> {
        if !self.index_path.exists() {
            return Ok(Index::new());
        }

        let content = fs::read_to_string(&self.index_path)
            .context("Failed to read index file")?;
        let index: Index = serde_json::from_str(&content)
            .context("Failed to parse index file")?;
        Ok(index)
    }

    pub fn save_index(&self, index: &Index) -> Result<()> {
        let mut content = serde_json::to_string_pretty(index)
            .context("Failed to serialize index")?;
        content.push('\n');
        fs::write(&self.index_path, content)
            .context("Failed to write index file")?;
        Ok(())
    }

    pub fn load_issue(&self, id: &Uuid) -> Result<Issue> {
        let index = self.load_index()?;
        let file_name = index.get_file_name(id)
            .ok_or_else(|| anyhow::anyhow!("Issue not found in index: {}", id))?;
        let path = self.issue_path(file_name);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read issue file: {}", file_name))?;
        let issue: Issue = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse issue file: {}", file_name))?;
        Ok(issue)
    }

    pub fn save_issue(&self, issue: &Issue, index: &mut Index) -> Result<()> {
        let new_file_name = issue.file_name();
        let old_file_name = index.get_file_name(&issue.id);

        // Handle file rename if title or type changed
        if let Some(old_name) = old_file_name {
            if old_name != new_file_name {
                let old_path = self.issue_path(old_name);
                if old_path.exists() {
                    fs::remove_file(&old_path)
                        .context("Failed to remove old issue file")?;
                }
                index.update_file_name(&issue.id, new_file_name.clone());
            }
        } else {
            // New issue
            index.add(issue.id, new_file_name.clone());
        }

        let path = self.issue_path(&new_file_name);
        let mut content = serde_json::to_string_pretty(issue)
            .context("Failed to serialize issue")?;
        content.push('\n');
        fs::write(&path, content)
            .with_context(|| format!("Failed to write issue file: {}", new_file_name))?;
        
        Ok(())
    }

    pub fn delete_issue(&self, id: &Uuid, index: &Index) -> Result<()> {
        if let Some(file_name) = index.get_file_name(id) {
            let path = self.issue_path(file_name);
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete issue file: {}", file_name))?;
            }
        }
        Ok(())
    }

    pub fn load_all_issues(&self) -> Result<Vec<Issue>> {
        let index = self.load_index()?;
        let mut issues = Vec::new();

        for metadata in &index.issues {
            let path = self.issue_path(&metadata.file_name);
            match fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_json::from_str::<Issue>(&content) {
                        Ok(issue) => issues.push(issue),
                        Err(e) => eprintln!("Warning: Failed to parse issue {}: {}", metadata.file_name, e),
                    }
                }
                Err(e) => eprintln!("Warning: Failed to load issue {}: {}", metadata.file_name, e),
            }
        }

        Ok(issues)
    }
}
