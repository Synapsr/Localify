//! Identity of an executable on disk, to notice that an update replaced it
//! while an older process keeps running.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryIdentity {
    /// Canonical path, so a symlink such as `/usr/local/bin/localify` and
    /// the app bundle binary it points to compare equal.
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    /// Modification time as (seconds, nanoseconds) since the Unix epoch.
    #[serde(default)]
    pub modified: Option<(u64, u32)>,
}

impl BinaryIdentity {
    pub fn of(path: &Path) -> Option<Self> {
        let path = fs::canonicalize(path).ok()?;
        let metadata = fs::metadata(&path).ok()?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| (duration.as_secs(), duration.subsec_nanos()));
        Some(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified,
        })
    }

    /// Identity of the running executable, as found on disk right now.
    pub fn current_executable() -> Option<Self> {
        Self::of(&std::env::current_exe().ok()?)
    }
}

/// Whether the running daemon uses the binary installed at a given path.
#[allow(dead_code)] // Used by the system helper through the library crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonBinaryCheck {
    NotRunning,
    Current,
    /// Same install, older build: an update or reinstall replaced the file.
    Outdated,
    /// Started from another binary (a dev build, another install): left alone.
    OtherInstall,
}

/// Compares the daemon's binary with the one installed now. `reported` comes
/// from the daemon; daemons older than this check do not report it, so their
/// executable path (from the process table) decides instead.
#[allow(dead_code)] // Used by the system helper through the library crate.
pub fn check_daemon_binary(
    installed: &BinaryIdentity,
    reported: Option<&BinaryIdentity>,
    reported_path: Option<&Path>,
) -> DaemonBinaryCheck {
    match reported {
        Some(identity) if identity.path != installed.path => DaemonBinaryCheck::OtherInstall,
        Some(identity) if identity == installed => DaemonBinaryCheck::Current,
        Some(_) => DaemonBinaryCheck::Outdated,
        None => {
            let same_path = reported_path
                .and_then(|path| fs::canonicalize(path).ok())
                .is_some_and(|path| path == installed.path);
            if same_path {
                DaemonBinaryCheck::Outdated
            } else {
                DaemonBinaryCheck::OtherInstall
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_file_changes_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("localify");
        fs::write(&binary, b"old build").unwrap();
        let before = BinaryIdentity::of(&binary).unwrap();

        let staged = dir.path().join("localify.new");
        fs::write(&staged, b"new build!").unwrap();
        fs::rename(&staged, &binary).unwrap();
        let after = BinaryIdentity::of(&binary).unwrap();

        assert_eq!(before.path, after.path);
        assert_ne!(before, after);
    }

    #[test]
    fn missing_files_have_no_identity() {
        let dir = tempfile::tempdir().unwrap();

        assert!(BinaryIdentity::of(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn symlinks_resolve_to_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("localify");
        fs::write(&binary, b"build").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&binary, &link).unwrap();

        assert_eq!(BinaryIdentity::of(&link), BinaryIdentity::of(&binary));
    }

    #[test]
    fn decides_whether_the_daemon_needs_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("localify");
        fs::write(&binary, b"new").unwrap();
        let installed = BinaryIdentity::of(&binary).unwrap();
        let older = BinaryIdentity {
            inode: installed.inode + 1,
            ..installed.clone()
        };
        let elsewhere = BinaryIdentity {
            path: dir.path().join("other"),
            ..installed.clone()
        };

        assert_eq!(
            check_daemon_binary(&installed, Some(&installed), None),
            DaemonBinaryCheck::Current
        );
        assert_eq!(
            check_daemon_binary(&installed, Some(&older), None),
            DaemonBinaryCheck::Outdated
        );
        assert_eq!(
            check_daemon_binary(&installed, Some(&elsewhere), None),
            DaemonBinaryCheck::OtherInstall
        );
        assert_eq!(
            check_daemon_binary(&installed, None, Some(&binary)),
            DaemonBinaryCheck::Outdated
        );
        assert_eq!(
            check_daemon_binary(&installed, None, Some(&dir.path().join("dev-build"))),
            DaemonBinaryCheck::OtherInstall
        );
        assert_eq!(
            check_daemon_binary(&installed, None, None),
            DaemonBinaryCheck::OtherInstall
        );
    }
}
