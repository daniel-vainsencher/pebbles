//! Project discovery for the TUI (r[project-registry]).
//! Pure parts (ordering, config parsing) are separated from filesystem walks.

use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A discovered pebbles project.
#[derive(Clone, Debug)]
pub struct Project {
    /// Directory containing `.pebbles/`.
    pub root: PathBuf,
    /// Directory name; used for display and tie-breaking.
    pub name: String,
}

/// Sort candidates by `.pebbles/index.json` mtime, most recent first;
/// missing index sorts last; ties order by name.
pub fn sort_projects(candidates: Vec<(PathBuf, Option<SystemTime>)>) -> Vec<Project> {
    let mut entries: Vec<(Project, Option<SystemTime>)> = candidates
        .into_iter()
        .map(|(root, mtime)| {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| root.to_string_lossy().to_string());
            (Project { root, name }, mtime)
        })
        .collect();
    entries.sort_by(|(pa, ta), (pb, tb)| {
        // Most recent index first; missing index last; ties by name.
        match (ta, tb) {
            (Some(a), Some(b)) => b.cmp(a).then_with(|| pa.name.cmp(&pb.name)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => pa.name.cmp(&pb.name),
        }
    });
    entries.into_iter().map(|(p, _)| p).collect()
}

/// Parse the extra-directories config (`~/.config/pebbles/projects.json`).
/// Absent or malformed config yields an empty list.
#[derive(Deserialize, Default)]
struct ProjectsConfig {
    #[serde(default)]
    extra_dirs: Vec<PathBuf>,
}

pub fn parse_extra_dirs(content: Option<&str>) -> Vec<PathBuf> {
    let Some(content) = content else {
        return Vec::new();
    };
    serde_json::from_str::<ProjectsConfig>(content)
        .map(|c| c.extra_dirs)
        .unwrap_or_default()
}

/// Discover projects: one level of `home/repos`, plus `extra_dirs`;
/// only directories containing `.pebbles/` count.
fn has_pebbles_dir(dir: &Path) -> bool {
    dir.join(".pebbles").is_dir()
}

pub fn index_mtime(root: &Path) -> Option<SystemTime> {
    std::fs::metadata(root.join(".pebbles").join("index.json"))
        .and_then(|m| m.modified())
        .ok()
}

/// Directories to scan: one level of `home/repos` plus the extra dirs.
pub fn scan_dirs(home: &Path, extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(home.join("repos"));
    dirs.extend(extra_dirs.iter().cloned());
    dirs
}

pub fn discover(home: &Path, extra_dirs: &[PathBuf]) -> Result<Vec<Project>> {
    let mut candidates: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();
    for scan in scan_dirs(home, extra_dirs) {
        let Ok(entries) = std::fs::read_dir(&scan) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !has_pebbles_dir(&path) {
                continue;
            }
            candidates.push((path.clone(), index_mtime(&path)));
        }
    }
    Ok(sort_projects(candidates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn ts(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn projects_sort_by_index_mtime_most_recent_first() {
        let a = PathBuf::from("/x/a");
        let b = PathBuf::from("/x/b");
        let c = PathBuf::from("/x/c");
        let sorted = sort_projects(vec![
            (b.clone(), Some(ts(100))),
            (a.clone(), Some(ts(200))),
            (c.clone(), None),
        ]);
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"], "newest index first, no-index last");
    }

    #[test]
    fn equal_mtime_ties_break_by_name() {
        let z = PathBuf::from("/x/zeta");
        let k = PathBuf::from("/x/kappa");
        let sorted = sort_projects(vec![(z, Some(ts(5))), (k, Some(ts(5)))]);
        let names: Vec<&str> = sorted.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["kappa", "zeta"]);
    }

    #[test]
    fn extra_dirs_config_parses_and_tolerates_absent_or_malformed() {
        assert_eq!(parse_extra_dirs(None), Vec::<PathBuf>::new());
        assert_eq!(parse_extra_dirs(Some("not json")), Vec::<PathBuf>::new());
        let dirs = parse_extra_dirs(Some(r#"{"extra_dirs": ["/p/one", "/p/two"]}"#));
        assert_eq!(dirs, vec![PathBuf::from("/p/one"), PathBuf::from("/p/two")]);
    }

    #[test]
    fn discover_scans_home_repos_and_extra_dirs_only_for_pebbles_dirs() {
        let tmp = tempfile::Builder::new()
            .prefix("pebbles-disc-")
            .tempdir()
            .unwrap();
        let home = tmp.path().to_path_buf();
        std::fs::create_dir_all(home.join("repos/alpha/.pebbles")).unwrap();
        std::fs::create_dir_all(home.join("repos/beta")).unwrap(); // no .pebbles
        let extra = home.join("elsewhere");
        std::fs::create_dir_all(extra.join("gamma/.pebbles")).unwrap();

        let projects = discover(&home, &[extra]).unwrap();
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "{names:?}");
        assert!(names.contains(&"gamma"), "{names:?}");
        assert!(
            !names.contains(&"beta"),
            "dir without .pebbles is not a project: {names:?}"
        );
    }
}
