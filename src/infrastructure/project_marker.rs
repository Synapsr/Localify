//! In-project marker file linking a folder back to its Localify project.
//!
//! Localify stores project root paths as absolute paths (config + local
//! project-roots metadata), which go stale when the user moves the folder.
//! The marker gives the folder its own identity so CLI and desktop flows can
//! re-link a moved project instead of losing it or creating a duplicate.
//!
//! A marker naming a project id that no longer exists (e.g. a committed
//! marker cloned on another machine) is treated the same as no marker.

use std::fs::{self, OpenOptions};
use std::io::Write;
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

/// The repository's own exclude file, shared by all its worktrees: the marker
/// never shows as a change, and nothing the project tracks, such as its
/// `.gitignore`, changes. A folder outside Git needs nothing.
fn ensure_marker_is_excluded(root: &Path) -> Result<()> {
    let Some(common_dir) = git_common_dir(root) else {
        return Ok(());
    };
    let exclude_path = common_dir.join("info").join("exclude");
    // `**/` also covers a project in a subfolder of the repository.
    let entry = format!("**/.{}/project.json", BRANDING.slug());
    let existing = match fs::read(&exclude_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read '{}'", exclude_path.display()));
        }
    };
    if String::from_utf8_lossy(&existing)
        .lines()
        .any(|line| line.trim() == entry)
    {
        return Ok(());
    }

    if let Some(info) = exclude_path.parent() {
        fs::create_dir_all(info)
            .with_context(|| format!("Failed to create '{}'", info.display()))?;
    }
    let mut exclude = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .with_context(|| format!("Failed to open '{}'", exclude_path.display()))?;
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        writeln!(exclude)
            .with_context(|| format!("Failed to update '{}'", exclude_path.display()))?;
    }
    writeln!(exclude, "{entry}")
        .with_context(|| format!("Failed to update '{}'", exclude_path.display()))?;
    Ok(())
}

/// The Git directory every worktree of the repository holding `root` shares,
/// found the way Git finds it, without running Git: the nearest `.git`, which
/// in a worktree is a file naming its own Git directory, whose `commondir`
/// names the shared one.
fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root
        .ancestors()
        .map(|folder| folder.join(".git"))
        .find(|path| path.exists())?;
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let content = fs::read_to_string(&dot_git).ok()?;
    let git_dir = dot_git
        .parent()?
        .join(content.strip_prefix("gitdir:")?.trim());
    match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => Some(git_dir.join(common.trim())),
        Err(_) => Some(git_dir),
    }
}

/// Records the project id inside the project folder. Overwrites any existing
/// marker and keeps the machine-local file out of Git, through the
/// repository's exclude file. Re-initializing a folder must always point it
/// at the new project.
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
    ensure_marker_is_excluded(root)?;
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

    fn exclude_entry() -> String {
        format!("**/.{}/project.json", BRANDING.slug())
    }

    #[test]
    fn write_then_read_round_trips_without_touching_the_project() {
        let temp = tempdir().unwrap();
        write_project_marker(temp.path(), "proj-123").unwrap();

        assert_eq!(
            read_project_marker(temp.path()),
            Some("proj-123".to_string())
        );
        // Outside Git there is nothing to keep it out of.
        assert!(!temp.path().join(".gitignore").exists());
    }

    #[test]
    fn write_overwrites_existing_marker() {
        let temp = tempdir().unwrap();
        write_project_marker(temp.path(), "old").unwrap();
        write_project_marker(temp.path(), "new").unwrap();

        assert_eq!(read_project_marker(temp.path()), Some("new".to_string()));
    }

    #[test]
    fn a_repository_excludes_the_marker_once_and_keeps_its_gitignore() {
        let temp = tempdir().unwrap();
        let info = temp.path().join(".git").join("info");
        fs::create_dir_all(&info).unwrap();
        fs::write(info.join("exclude"), "# local patterns").unwrap();
        fs::write(temp.path().join(".gitignore"), "node_modules\n").unwrap();
        let app = temp.path().join("apps/web");
        fs::create_dir_all(&app).unwrap();

        write_project_marker(temp.path(), "proj-123").unwrap();
        write_project_marker(&app, "proj-456").unwrap();

        assert_eq!(
            fs::read_to_string(info.join("exclude")).unwrap(),
            format!("# local patterns\n{}\n", exclude_entry())
        );
        assert_eq!(
            fs::read_to_string(temp.path().join(".gitignore")).unwrap(),
            "node_modules\n"
        );
    }

    #[test]
    fn a_worktree_excludes_the_marker_for_the_whole_repository() {
        let temp = tempdir().unwrap();
        let common = temp.path().join("repo").join(".git");
        let worktree_git = common.join("worktrees").join("feature");
        fs::create_dir_all(&worktree_git).unwrap();
        fs::write(worktree_git.join("commondir"), "../..\n").unwrap();
        let worktree = temp.path().join("feature");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();

        write_project_marker(&worktree, "proj-123").unwrap();

        let exclude = fs::read_to_string(common.join("info").join("exclude")).unwrap();
        assert_eq!(exclude, format!("{}\n", exclude_entry()));
        assert!(!worktree_git.join("info").exists());
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
