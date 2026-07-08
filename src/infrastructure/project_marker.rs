//! In-project marker file linking a folder back to its Localify project.
//!
//! Localify stores project root paths as absolute paths (config + local
//! project-roots metadata), which go stale when the user moves the folder.
//! The marker gives the folder its own identity so CLI and desktop flows can
//! re-link a moved project instead of losing it or creating a duplicate.
//!
//! A marker naming a project id that no longer exists (e.g. a committed
//! marker cloned on another machine) is treated the same as no marker.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::branding::BRANDING;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectMarkerFile {
    project_id: String,
}

/// Path of the marker file relative to the project root, e.g.
/// `.localify/project.json`. The directory follows the active branding
/// profile so dev-profile projects never collide with production ones.
pub fn project_marker_relative_path() -> PathBuf {
    PathBuf::from(format!(".{}", BRANDING.slug())).join("project.json")
}

fn marker_path(root: &Path) -> PathBuf {
    root.join(project_marker_relative_path())
}

/// Records the project id inside the project folder. Overwrites any existing
/// marker: re-initializing a folder must always point it at the new project.
pub fn write_project_marker(root: &Path, project_id: &str) -> Result<()> {
    let path = marker_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create '{}'", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(&ProjectMarkerFile {
        project_id: project_id.to_string(),
    })
    .context("Failed to serialize the project marker")?;
    fs::write(&path, format!("{content}\n"))
        .with_context(|| format!("Failed to write project marker '{}'", path.display()))?;
    Ok(())
}

/// Reads the project id recorded in `root`, if any. Malformed or empty
/// markers are treated as absent so a corrupted file never blocks a command.
pub fn read_project_marker(root: &Path) -> Option<String> {
    let content = fs::read_to_string(marker_path(root)).ok()?;
    let marker: ProjectMarkerFile = serde_json::from_str(&content).ok()?;
    let project_id = marker.project_id.trim();
    if project_id.is_empty() {
        return None;
    }
    Some(project_id.to_string())
}

/// Walks up from `start` looking for a marker, like Git looks for `.git`.
/// Returns the folder holding the marker (the project root) and the recorded
/// project id.
pub fn find_project_marker(start: &Path) -> Option<(PathBuf, String)> {
    start.ancestors().find_map(|ancestor| {
        read_project_marker(ancestor).map(|project_id| (ancestor.to_path_buf(), project_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_then_read_round_trips() {
        let temp = tempdir().unwrap();
        write_project_marker(temp.path(), "proj-123").unwrap();

        assert_eq!(
            read_project_marker(temp.path()),
            Some("proj-123".to_string())
        );
    }

    #[test]
    fn write_overwrites_existing_marker() {
        let temp = tempdir().unwrap();
        write_project_marker(temp.path(), "old").unwrap();
        write_project_marker(temp.path(), "new").unwrap();

        assert_eq!(read_project_marker(temp.path()), Some("new".to_string()));
    }

    #[test]
    fn read_ignores_missing_malformed_and_empty_markers() {
        let temp = tempdir().unwrap();
        assert_eq!(read_project_marker(temp.path()), None);

        let path = marker_path(temp.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert_eq!(read_project_marker(temp.path()), None);

        fs::write(&path, r#"{"projectId": "  "}"#).unwrap();
        assert_eq!(read_project_marker(temp.path()), None);
    }

    #[test]
    fn find_walks_up_to_the_marker_root() {
        let temp = tempdir().unwrap();
        let nested = temp.path().join("apps/web");
        fs::create_dir_all(&nested).unwrap();
        write_project_marker(temp.path(), "proj-123").unwrap();

        let (root, project_id) = find_project_marker(&nested).unwrap();
        assert_eq!(root, temp.path());
        assert_eq!(project_id, "proj-123");
    }

    #[test]
    fn find_returns_none_without_marker() {
        let temp = tempdir().unwrap();
        assert_eq!(find_project_marker(temp.path()), None);
    }
}
