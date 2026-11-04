//! Identity of an executable on disk, to notice that an update replaced it
//! while an older process keeps running.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryIdentity {
    /// Canonical path, so a symlink such as `/usr/local/bin/localify` and
    /// the app bundle binary it points to compare equal.
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    /// Modification time as (seconds, nanoseconds) since the Unix epoch.
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
    fn symlinks_resolve_to_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("localify");
        fs::write(&binary, b"build").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&binary, &link).unwrap();

        assert_eq!(BinaryIdentity::of(&link), BinaryIdentity::of(&binary));
    }
}
