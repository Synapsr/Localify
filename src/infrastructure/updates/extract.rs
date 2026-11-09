//! Expands a release archive into the installer's private staging
//! directory, refusing anything a Localify release never holds
//! (docs/desktop.md, "Automatic updates", "Extraction").
//!
//! Nothing here follows a symlink while writing: an entry under a symlink is
//! refused, files are created with `O_NOFOLLOW` and `create_new`, and every
//! symlink is checked once the whole archive is out.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::EntryType;

/// The one top-level item a release archive holds.
pub const BUNDLE_NAME: &str = "Localify.app";

/// How much an archive may expand to.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_entries: usize,
}

/// 1 GiB and 20,000 entries: a release is about 40 MB and 20 entries.
pub const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024 * 1024,
    max_entries: 20_000,
};

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("the archive has an absolute path: {0}")]
    AbsolutePath(String),
    #[error("the archive has a path with '..': {0}")]
    ParentComponent(String),
    #[error("the archive holds something besides one {BUNDLE_NAME}: {0}")]
    OutsideBundle(String),
    #[error("the archive has no {BUNDLE_NAME}")]
    NoBundle,
    #[error("the archive has a hard link: {0}")]
    HardLink(String),
    #[error("the archive has a device, FIFO or other special file: {0}")]
    SpecialFile(String),
    #[error("the archive has a setuid or setgid file: {0}")]
    SetId(String),
    #[error("the archive has a symlink that leaves the bundle: {0}")]
    SymlinkEscapes(String),
    #[error("the archive puts an entry under a symlink: {0}")]
    UnderSymlink(String),
    #[error("the archive lists an entry twice: {0}")]
    Duplicate(String),
    #[error("the archive expands past {} MiB", LIMITS.max_bytes / (1024 * 1024))]
    TooBig,
    #[error("the archive has more than {} entries", LIMITS.max_entries)]
    TooManyEntries,
    #[error("the archive could not be read: {0}")]
    Archive(io::Error),
    #[error("could not write the staged bundle: {0}")]
    Write(io::Error),
}

/// Expands `archive` (a `.tar.gz`) into `staging`, which must be a new empty
/// directory only this process writes, and returns the bundle's path.
pub fn extract_bundle(
    archive: &[u8],
    staging: &Path,
    limits: Limits,
) -> Result<PathBuf, ExtractError> {
    let mut tar = tar::Archive::new(GzDecoder::new(archive));
    let mut state = Extraction::new(staging, limits);
    for entry in tar.entries().map_err(ExtractError::Archive)? {
        let entry = entry.map_err(ExtractError::Archive)?;
        state.add(entry)?;
    }
    state.finish()
}

struct Extraction<'a> {
    staging: &'a Path,
    limits: Limits,
    entries: usize,
    bytes: u64,
    /// Relative paths of the symlinks written so far.
    symlinks: HashSet<PathBuf>,
    /// Relative paths of every entry written so far.
    written: HashSet<PathBuf>,
}

impl<'a> Extraction<'a> {
    fn new(staging: &'a Path, limits: Limits) -> Self {
        Self {
            staging,
            limits,
            entries: 0,
            bytes: 0,
            symlinks: HashSet::new(),
            written: HashSet::new(),
        }
    }

    fn add<R: Read>(&mut self, mut entry: tar::Entry<'_, R>) -> Result<(), ExtractError> {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(ExtractError::TooManyEntries);
        }
        let raw = entry.path_bytes().into_owned();
        let shown = String::from_utf8_lossy(&raw).into_owned();
        let relative = bundle_relative_path(&raw)?;
        if relative
            .ancestors()
            .skip(1)
            .any(|a| self.symlinks.contains(a))
        {
            return Err(ExtractError::UnderSymlink(shown));
        }
        let mode = entry.header().mode().map_err(ExtractError::Archive)?;
        if mode & 0o6000 != 0 {
            return Err(ExtractError::SetId(shown));
        }
        let kind = entry.header().entry_type();
        let is_dir = kind == EntryType::Directory;
        if !self.written.insert(relative.clone()) && !is_dir {
            return Err(ExtractError::Duplicate(shown));
        }
        let target = self.staging.join(&relative);
        match kind {
            EntryType::Directory => make_dir(&target, mode),
            EntryType::Regular => self.write_file(&mut entry, &target, mode),
            EntryType::Symlink => {
                let link = entry
                    .link_name_bytes()
                    .map(|name| name.into_owned())
                    .ok_or_else(|| ExtractError::SymlinkEscapes(shown.clone()))?;
                check_symlink_target(&relative, &link)
                    .ok_or_else(|| ExtractError::SymlinkEscapes(shown.clone()))?;
                make_parent(&target)?;
                std::os::unix::fs::symlink(OsStr::from_bytes(&link), &target)
                    .map_err(ExtractError::Write)?;
                self.symlinks.insert(relative);
                Ok(())
            }
            EntryType::Link => Err(ExtractError::HardLink(shown)),
            _ => Err(ExtractError::SpecialFile(shown)),
        }
    }

    fn write_file<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        target: &Path,
        mode: u32,
    ) -> Result<(), ExtractError> {
        let size = entry.size();
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.limits.max_bytes {
            return Err(ExtractError::TooBig);
        }
        make_parent(target)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o600)
            .open(target)
            .map_err(ExtractError::Write)?;
        let copied = io::copy(&mut entry.take(size), &mut file).map_err(ExtractError::Archive)?;
        if copied != size {
            return Err(ExtractError::Archive(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "an entry is shorter than its header says",
            )));
        }
        file.set_permissions(Permissions::from_mode(mode & 0o755))
            .map_err(ExtractError::Write)
    }

    /// Checks what only the whole archive shows: the bundle is there, and
    /// every symlink, followed through the others, stays inside it.
    fn finish(self) -> Result<PathBuf, ExtractError> {
        let bundle = self.staging.join(BUNDLE_NAME);
        let is_dir = fs::symlink_metadata(&bundle).is_ok_and(|meta| meta.file_type().is_dir());
        if !is_dir {
            return Err(ExtractError::NoBundle);
        }
        let root = fs::canonicalize(&bundle).map_err(ExtractError::Write)?;
        for link in &self.symlinks {
            let resolved = fs::canonicalize(self.staging.join(link));
            if !resolved.is_ok_and(|path| path.starts_with(&root)) {
                return Err(ExtractError::SymlinkEscapes(link.display().to_string()));
            }
        }
        Ok(bundle)
    }
}

/// The entry's path with `./` dropped, refused when it is absolute, has
/// `..`, or lies outside `Localify.app`.
fn bundle_relative_path(raw: &[u8]) -> Result<PathBuf, ExtractError> {
    let shown = || String::from_utf8_lossy(raw).into_owned();
    let path = Path::new(OsStr::from_bytes(raw));
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err(ExtractError::ParentComponent(shown())),
            Component::RootDir | Component::Prefix(_) => {
                return Err(ExtractError::AbsolutePath(shown()));
            }
        }
    }
    match relative.components().next() {
        Some(Component::Normal(first)) if first == BUNDLE_NAME => Ok(relative),
        _ => Err(ExtractError::OutsideBundle(shown())),
    }
}

/// `Some` when a symlink at `link_path` (relative to the staging directory)
/// pointing at `target` stays inside the bundle, read without the filesystem.
fn check_symlink_target(link_path: &Path, target: &[u8]) -> Option<()> {
    let target = Path::new(OsStr::from_bytes(target));
    if target.as_os_str().is_empty() {
        return None;
    }
    // The bundle's own name is the floor: the stack never goes below it.
    let mut stack: Vec<&OsStr> = link_path
        .parent()?
        .components()
        .map(Component::as_os_str)
        .collect();
    for component in target.components() {
        match component {
            Component::Normal(part) => stack.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop();
                if stack.is_empty() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(())
}

fn make_parent(target: &Path) -> Result<(), ExtractError> {
    match target.parent() {
        Some(parent) => DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(parent)
            .map_err(ExtractError::Write),
        None => Ok(()),
    }
}

fn make_dir(target: &Path, mode: u32) -> Result<(), ExtractError> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => {
            return Err(ExtractError::Duplicate(target.display().to_string()));
        }
        Err(_) => {
            make_parent(target)?;
            DirBuilder::new()
                .mode(0o755)
                .create(target)
                .map_err(ExtractError::Write)?;
        }
    }
    // The owner keeps write access, so the staging dir can be cleaned up.
    fs::set_permissions(target, Permissions::from_mode((mode & 0o755) | 0o700))
        .map_err(ExtractError::Write)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    /// One entry of a crafted archive.
    pub(crate) enum Item<'a> {
        Dir(&'a str),
        File(&'a str, &'a [u8], u32),
        Symlink(&'a str, &'a str),
        HardLink(&'a str, &'a str),
        Fifo(&'a str),
    }

    /// Builds a `.tar.gz` with raw headers, so it can hold what `tar`'s
    /// builder refuses to write, such as `..` and absolute paths.
    pub(crate) fn archive(items: &[Item<'_>]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for item in items {
            let (path, kind, mode, data, link): (&str, EntryType, u32, &[u8], Option<&str>) =
                match *item {
                    Item::Dir(path) => (path, EntryType::Directory, 0o755, b"", None),
                    Item::File(path, data, mode) => (path, EntryType::Regular, mode, data, None),
                    Item::Symlink(path, to) => (path, EntryType::Symlink, 0o755, b"", Some(to)),
                    Item::HardLink(path, to) => (path, EntryType::Link, 0o644, b"", Some(to)),
                    Item::Fifo(path) => (path, EntryType::Fifo, 0o644, b"", None),
                };
            let mut header = tar::Header::new_gnu();
            raw_name(&mut header.as_old_mut().name, path);
            if let Some(link) = link {
                raw_name(&mut header.as_old_mut().linkname, link);
            }
            header.set_entry_type(kind);
            header.set_mode(mode);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn raw_name(field: &mut [u8; 100], value: &str) {
        field.fill(0);
        field[..value.len()].copy_from_slice(value.as_bytes());
    }

    pub(crate) fn good_bundle() -> Vec<u8> {
        archive(&[
            Item::Dir("Localify.app/"),
            Item::Dir("Localify.app/Contents/"),
            Item::File("Localify.app/Contents/Info.plist", b"<plist/>", 0o644),
            Item::File("Localify.app/Contents/MacOS/localify", b"bin", 0o755),
            Item::Symlink("Localify.app/Contents/Current", "MacOS"),
        ])
    }

    fn extract(bytes: &[u8]) -> (tempfile::TempDir, Result<PathBuf, ExtractError>) {
        extract_with(bytes, LIMITS)
    }

    fn extract_with(
        bytes: &[u8],
        limits: Limits,
    ) -> (tempfile::TempDir, Result<PathBuf, ExtractError>) {
        let dir = tempfile::tempdir().unwrap();
        let result = extract_bundle(bytes, dir.path(), limits);
        (dir, result)
    }

    #[test]
    fn a_plain_bundle_expands_with_its_modes() {
        let (dir, result) = extract(&good_bundle());
        let bundle = result.unwrap();
        assert_eq!(bundle, dir.path().join("Localify.app"));
        let binary = bundle.join("Contents/MacOS/localify");
        assert_eq!(fs::read(&binary).unwrap(), b"bin");
        let mode = fs::metadata(&binary).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        let link = fs::read_link(bundle.join("Contents/Current")).unwrap();
        assert_eq!(link, Path::new("MacOS"));
    }

    #[test]
    fn a_leading_dot_slash_is_accepted() {
        let bytes = archive(&[Item::File(
            "./Localify.app/Contents/Info.plist",
            b"x",
            0o644,
        )]);
        assert!(extract(&bytes).1.is_ok());
    }

    #[test]
    fn traversal_is_refused_before_anything_is_written() {
        let bytes = archive(&[Item::File("Localify.app/../../evil", b"x", 0o644)]);
        let (dir, result) = extract(&bytes);
        assert!(matches!(result, Err(ExtractError::ParentComponent(_))));
        assert!(!dir.path().join("../evil").exists());
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let bytes = archive(&[Item::File("/tmp/Localify.app/evil", b"x", 0o644)]);
        assert!(matches!(
            extract(&bytes).1,
            Err(ExtractError::AbsolutePath(_))
        ));
    }

    #[test]
    fn a_hard_link_is_refused() {
        let bytes = archive(&[
            Item::File("Localify.app/a", b"x", 0o644),
            Item::HardLink("Localify.app/b", "Localify.app/a"),
        ]);
        assert!(matches!(extract(&bytes).1, Err(ExtractError::HardLink(_))));
    }

    #[test]
    fn a_fifo_is_refused() {
        let bytes = archive(&[Item::Fifo("Localify.app/pipe")]);
        assert!(matches!(
            extract(&bytes).1,
            Err(ExtractError::SpecialFile(_))
        ));
    }

    #[test]
    fn setuid_and_setgid_files_are_refused() {
        for mode in [0o4755, 0o2755] {
            let bytes = archive(&[Item::File("Localify.app/Contents/MacOS/x", b"x", mode)]);
            assert!(matches!(extract(&bytes).1, Err(ExtractError::SetId(_))));
        }
    }

    #[test]
    fn symlinks_that_leave_the_bundle_are_refused() {
        for target in ["../../outside", "/etc/passwd", "..", "../Other.app"] {
            let bytes = archive(&[Item::Symlink("Localify.app/link", target)]);
            assert!(
                matches!(extract(&bytes).1, Err(ExtractError::SymlinkEscapes(_))),
                "{target} was accepted"
            );
        }
    }

    #[test]
    fn a_symlink_chain_that_leaves_the_bundle_is_refused() {
        // Each link stays inside on its own; through `here`, `out` reaches
        // the staging directory.
        let bytes = archive(&[
            Item::Dir("Localify.app/"),
            Item::Symlink("Localify.app/here", "."),
            Item::Symlink("Localify.app/out", "here/.."),
        ]);
        assert!(matches!(
            extract(&bytes).1,
            Err(ExtractError::SymlinkEscapes(_))
        ));
    }

    #[test]
    fn an_entry_under_a_symlink_is_refused() {
        let bytes = archive(&[
            Item::Dir("Localify.app/Contents/"),
            Item::Symlink("Localify.app/Contents/Link", "."),
            Item::File("Localify.app/Contents/Link/file", b"x", 0o644),
        ]);
        assert!(matches!(
            extract(&bytes).1,
            Err(ExtractError::UnderSymlink(_))
        ));
    }

    #[test]
    fn two_bundles_or_a_stray_file_are_refused() {
        let two = archive(&[
            Item::File("Localify.app/a", b"x", 0o644),
            Item::File("Other.app/a", b"x", 0o644),
        ]);
        assert!(matches!(
            extract(&two).1,
            Err(ExtractError::OutsideBundle(_))
        ));
        let stray = archive(&[Item::File("README", b"x", 0o644)]);
        assert!(matches!(
            extract(&stray).1,
            Err(ExtractError::OutsideBundle(_))
        ));
    }

    #[test]
    fn a_file_listed_twice_is_refused() {
        let bytes = archive(&[
            Item::File("Localify.app/a", b"x", 0o644),
            Item::File("Localify.app/a", b"y", 0o644),
        ]);
        assert!(matches!(extract(&bytes).1, Err(ExtractError::Duplicate(_))));
    }

    #[test]
    fn an_archive_past_the_size_cap_is_refused() {
        let bytes = archive(&[Item::File("Localify.app/big", &[0u8; 4096], 0o644)]);
        let limits = Limits {
            max_bytes: 4095,
            max_entries: 10,
        };
        assert!(matches!(
            extract_with(&bytes, limits).1,
            Err(ExtractError::TooBig)
        ));
    }

    #[test]
    fn an_archive_past_the_entry_cap_is_refused() {
        let limits = Limits {
            max_bytes: 1024,
            max_entries: 3,
        };
        assert!(matches!(
            extract_with(&good_bundle(), limits).1,
            Err(ExtractError::TooManyEntries)
        ));
    }

    #[test]
    fn an_empty_archive_has_no_bundle() {
        assert!(matches!(
            extract(&archive(&[])).1,
            Err(ExtractError::NoBundle)
        ));
    }

    #[test]
    fn garbage_is_an_archive_error() {
        assert!(matches!(
            extract(b"not a gzip").1,
            Err(ExtractError::Archive(_))
        ));
    }
}
