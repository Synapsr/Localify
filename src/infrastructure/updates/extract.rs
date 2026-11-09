//! Expands a release archive into the installer's private staging
//! directory, refusing anything a Localify release never holds
//! (docs/desktop.md, "Automatic updates", "Extraction").
//!
//! Nothing here follows a symlink while writing. Every path is walked one
//! component at a time from a directory descriptor, with `O_NOFOLLOW` at
//! each step, and files are created with `O_EXCL`. Symlinks are made only
//! once every file and directory is out, then checked through each other.
//! That holds on a case-insensitive volume too, where `UP` and `up` are the
//! same name.
//!
//! The archive is read in tar's raw mode, so every header counts toward the
//! limits, including the long-name and PAX headers tar would otherwise read
//! whole and hide. Those are read here, capped.

use std::collections::{BTreeMap, HashSet};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::{EntryType, PaxExtensions};

/// The one top-level item a release archive holds.
pub const BUNDLE_NAME: &str = "Localify.app";

/// How much an archive may expand to.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The files' contents, together.
    pub max_bytes: u64,
    /// Every tar header, long-name and PAX headers included.
    pub max_entries: usize,
    /// Long-name and PAX headers' contents, together.
    pub max_metadata_bytes: u64,
}

/// 1 GiB, 20,000 headers and 1 MiB of metadata: a release is about 40 MB,
/// 22 headers and 2 KiB of PAX records.
pub const LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024 * 1024,
    max_entries: 20_000,
    max_metadata_bytes: 1024 * 1024,
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
    #[error("the archive has two entries at the same place: {0}")]
    Duplicate(String),
    #[error("the archive has data in an entry that is not a file: {0}")]
    Payload(String),
    #[error("the archive has tar metadata a release never holds: {0}")]
    Metadata(String),
    #[error("the archive's tar metadata is over {} KiB", LIMITS.max_metadata_bytes / 1024)]
    MetadataTooBig,
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
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(staging)
        .map_err(ExtractError::Write)?;
    let stream = Capped::new(GzDecoder::new(archive), stream_cap(limits));
    let mut tar = tar::Archive::new(stream);
    let mut state = Extraction::new(OwnedFd::from(root), limits);
    let read = state.read_entries(&mut tar);
    // Past the cap, tar reports the reader's error as its own.
    if tar.into_inner().exceeded {
        return Err(ExtractError::TooBig);
    }
    read?;
    state.make_symlinks()?;
    state.finish(staging)
}

/// The most the decompressed stream may hold: the contents, the metadata,
/// and a header plus padding for every entry, with the closing blocks.
fn stream_cap(limits: Limits) -> u64 {
    let per_entry = 2 * 512;
    limits
        .max_bytes
        .saturating_add(limits.max_metadata_bytes)
        .saturating_add((limits.max_entries as u64 + 2).saturating_mul(per_entry))
}

/// A reader that fails once more than its cap would be read, so a small
/// archive can't expand without bound before any entry is looked at.
struct Capped<R> {
    inner: R,
    left: u64,
    exceeded: bool,
}

impl<R> Capped<R> {
    fn new(inner: R, cap: u64) -> Self {
        Self {
            inner,
            left: cap,
            exceeded: false,
        }
    }
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            // At the cap, only the end of the stream is still fine.
            let mut probe = [0u8; 1];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            self.exceeded = true;
            return Err(io::Error::other("the archive expands past its cap"));
        }
        let room = usize::try_from(self.left).unwrap_or(usize::MAX);
        let take = buf.len().min(room);
        let read = self.inner.read(&mut buf[..take])?;
        self.left -= read as u64;
        Ok(read)
    }
}

/// A long name or PAX path that applies to the next entry.
#[derive(Default)]
struct PendingNames {
    path: Option<Vec<u8>>,
    link: Option<Vec<u8>>,
}

struct Extraction {
    /// The staging directory, opened once.
    root: OwnedFd,
    limits: Limits,
    entries: usize,
    bytes: u64,
    metadata_bytes: u64,
    /// Symlinks to make once everything else is out: relative path and
    /// target.
    symlinks: BTreeMap<PathBuf, Vec<u8>>,
    /// Relative paths of every entry seen so far.
    written: HashSet<PathBuf>,
}

impl Extraction {
    fn new(root: OwnedFd, limits: Limits) -> Self {
        Self {
            root,
            limits,
            entries: 0,
            bytes: 0,
            metadata_bytes: 0,
            symlinks: BTreeMap::new(),
            written: HashSet::new(),
        }
    }

    fn read_entries<R: Read>(&mut self, tar: &mut tar::Archive<R>) -> Result<(), ExtractError> {
        let mut names = PendingNames::default();
        let entries = tar.entries().map_err(ExtractError::Archive)?.raw(true);
        for entry in entries {
            let mut entry = entry.map_err(ExtractError::Archive)?;
            self.entries += 1;
            if self.entries > self.limits.max_entries {
                return Err(ExtractError::TooManyEntries);
            }
            if self.read_metadata(&mut entry, &mut names)? {
                continue;
            }
            let path = match names.path.take() {
                Some(path) => path,
                None => entry.path_bytes().into_owned(),
            };
            let link = match names.link.take() {
                Some(link) => Some(link),
                None => entry.link_name_bytes().map(|link| link.into_owned()),
            };
            self.add(&mut entry, &path, link)?;
        }
        if names.path.is_some() || names.link.is_some() {
            return Err(ExtractError::Metadata("a long name with no entry".into()));
        }
        Ok(())
    }

    /// Reads a GNU long-name or a PAX header into `names`, within the
    /// metadata cap. `false` for any other entry. Of PAX records, only
    /// `path` and `linkpath` are used; `size` and sparse records, which make
    /// tar readers disagree on where entries start, are refused.
    fn read_metadata<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        names: &mut PendingNames,
    ) -> Result<bool, ExtractError> {
        let kind = entry.header().entry_type();
        let long_name = kind.is_gnu_longname();
        let long_link = kind.is_gnu_longlink();
        let pax = kind.is_pax_local_extensions();
        if kind.is_pax_global_extensions() {
            return Err(ExtractError::Metadata("a global PAX header".into()));
        }
        if !(long_name || long_link || pax) {
            return Ok(false);
        }
        let size = entry.size();
        self.metadata_bytes = self.metadata_bytes.saturating_add(size);
        if self.metadata_bytes > self.limits.max_metadata_bytes {
            return Err(ExtractError::MetadataTooBig);
        }
        let mut data = Vec::new();
        entry
            .take(size)
            .read_to_end(&mut data)
            .map_err(ExtractError::Archive)?;
        if pax {
            read_pax(&data, names)?;
        } else {
            while data.last() == Some(&0) {
                data.pop();
            }
            let slot = if long_name {
                &mut names.path
            } else {
                &mut names.link
            };
            set_once(slot, data)?;
        }
        Ok(true)
    }

    fn add<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        raw: &[u8],
        link: Option<Vec<u8>>,
    ) -> Result<(), ExtractError> {
        let shown = String::from_utf8_lossy(raw).into_owned();
        let relative = bundle_relative_path(raw)?;
        if relative
            .ancestors()
            .skip(1)
            .any(|a| self.symlinks.contains_key(a))
        {
            return Err(ExtractError::UnderSymlink(shown));
        }
        let mode = entry.header().mode().map_err(ExtractError::Archive)?;
        if mode & 0o6000 != 0 {
            return Err(ExtractError::SetId(shown));
        }
        let kind = entry.header().entry_type();
        if kind != EntryType::Regular && entry.size() != 0 {
            return Err(ExtractError::Payload(shown));
        }
        let is_dir = kind == EntryType::Directory;
        if !self.written.insert(relative.clone()) && !is_dir {
            return Err(ExtractError::Duplicate(shown));
        }
        match kind {
            EntryType::Directory => self.make_dir(&relative, mode, &shown),
            EntryType::Regular => self.write_file(entry, &relative, mode, &shown),
            EntryType::Symlink => {
                let link = link.ok_or_else(|| ExtractError::SymlinkEscapes(shown.clone()))?;
                check_symlink_target(&relative, &link)
                    .ok_or_else(|| ExtractError::SymlinkEscapes(shown.clone()))?;
                self.symlinks.insert(relative, link);
                Ok(())
            }
            EntryType::Link => Err(ExtractError::HardLink(shown)),
            _ => Err(ExtractError::SpecialFile(shown)),
        }
    }

    fn make_dir(&self, relative: &Path, mode: u32, shown: &str) -> Result<(), ExtractError> {
        let (parent, name) = self.parent_dir(relative, shown)?;
        match mkdir_at(parent.as_fd(), &name) {
            Ok(()) => {}
            // Listed twice, or made earlier as some entry's parent.
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {}
            Err(error) => return Err(placement_error(error, shown)),
        }
        let dir = open_dir_at(parent.as_fd(), &name).map_err(|e| placement_error(e, shown))?;
        // The owner keeps write access, so the staging dir can be cleaned up.
        File::from(dir)
            .set_permissions(Permissions::from_mode((mode & 0o755) | 0o700))
            .map_err(ExtractError::Write)
    }

    fn write_file<R: Read>(
        &mut self,
        entry: &mut tar::Entry<'_, R>,
        relative: &Path,
        mode: u32,
        shown: &str,
    ) -> Result<(), ExtractError> {
        let size = entry.size();
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > self.limits.max_bytes {
            return Err(ExtractError::TooBig);
        }
        let (parent, name) = self.parent_dir(relative, shown)?;
        let file = create_file_at(parent.as_fd(), &name).map_err(|e| placement_error(e, shown))?;
        let mut file = File::from(file);
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

    /// Makes the symlinks, once nothing else will be written.
    fn make_symlinks(&self) -> Result<(), ExtractError> {
        for (relative, target) in &self.symlinks {
            let shown = relative.display().to_string();
            let (parent, name) = self.parent_dir(relative, &shown)?;
            let target = c_name(target)?;
            symlink_at(&target, parent.as_fd(), &name).map_err(|e| placement_error(e, &shown))?;
        }
        Ok(())
    }

    /// Opens the directory that holds `relative`'s last component, making
    /// the missing ones, and never through a symlink.
    fn parent_dir(&self, relative: &Path, shown: &str) -> Result<(OwnedFd, CString), ExtractError> {
        let mut parts: Vec<&OsStr> = relative.iter().collect();
        let last = parts
            .pop()
            .ok_or_else(|| ExtractError::OutsideBundle(shown.to_string()))?;
        let mut dir = self.root.try_clone().map_err(ExtractError::Write)?;
        for part in parts {
            let name = c_name(part.as_bytes())?;
            let opened = match open_dir_at(dir.as_fd(), &name) {
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                    match mkdir_at(dir.as_fd(), &name) {
                        Err(error) if error.raw_os_error() != Some(libc::EEXIST) => Err(error),
                        _ => open_dir_at(dir.as_fd(), &name),
                    }
                }
                other => other,
            };
            dir = opened.map_err(|error| placement_error(error, shown))?;
        }
        Ok((dir, c_name(last.as_bytes())?))
    }

    /// Checks what only the whole archive shows: the bundle is there, and
    /// every symlink, followed through the others, stays inside it.
    fn finish(self, staging: &Path) -> Result<PathBuf, ExtractError> {
        let bundle = staging.join(BUNDLE_NAME);
        let is_dir = fs::symlink_metadata(&bundle).is_ok_and(|meta| meta.file_type().is_dir());
        if !is_dir {
            return Err(ExtractError::NoBundle);
        }
        let root = fs::canonicalize(&bundle).map_err(ExtractError::Write)?;
        for link in self.symlinks.keys() {
            let resolved = fs::canonicalize(staging.join(link));
            if !resolved.is_ok_and(|path| path.starts_with(&root)) {
                return Err(ExtractError::SymlinkEscapes(link.display().to_string()));
            }
        }
        Ok(bundle)
    }
}

/// Takes `path` and `linkpath` from PAX records, and refuses the records
/// that change where entries start.
fn read_pax(data: &[u8], names: &mut PendingNames) -> Result<(), ExtractError> {
    for record in PaxExtensions::new(data) {
        let record = record.map_err(ExtractError::Archive)?;
        let key = record.key_bytes();
        if key == b"path" {
            set_once(&mut names.path, record.value_bytes().to_vec())?;
        } else if key == b"linkpath" {
            set_once(&mut names.link, record.value_bytes().to_vec())?;
        } else if key == b"size" || key.starts_with(b"GNU.sparse") {
            let key = String::from_utf8_lossy(key);
            return Err(ExtractError::Metadata(format!("a PAX '{key}' record")));
        }
    }
    Ok(())
}

fn set_once(slot: &mut Option<Vec<u8>>, value: Vec<u8>) -> Result<(), ExtractError> {
    if slot.is_some() {
        return Err(ExtractError::Metadata("two names for one entry".into()));
    }
    *slot = Some(value);
    Ok(())
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

/// What a failure to place an entry means: a symlink in the way, or two
/// entries at the same place (on a case-insensitive volume, `a` and `A`).
fn placement_error(error: io::Error, shown: &str) -> ExtractError {
    match error.raw_os_error() {
        Some(libc::ELOOP) => ExtractError::UnderSymlink(shown.to_string()),
        Some(libc::EEXIST) | Some(libc::ENOTDIR) => ExtractError::Duplicate(shown.to_string()),
        _ => ExtractError::Write(error),
    }
}

fn c_name(bytes: &[u8]) -> Result<CString, ExtractError> {
    CString::new(bytes).map_err(|_| {
        ExtractError::Archive(io::Error::new(
            io::ErrorKind::InvalidData,
            "a name holds a NUL byte",
        ))
    })
}

/// `fd` from a call that returns a new descriptor or -1.
fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by a successful `openat`, so it is an
    // open descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens the directory `name` in `dir`, failing on a symlink (`ELOOP`) or
/// anything but a directory (`ENOTDIR`).
fn open_dir_at(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: `dir` is an open descriptor and `name` a NUL-terminated
    // string, both alive for the call, which only reads `name`.
    owned(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) })
}

/// Creates the file `name` in `dir`, failing when anything, a symlink
/// included, is already there.
fn create_file_at(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mode: libc::c_uint = 0o600;
    // SAFETY: as in `open_dir_at`; with `O_CREAT`, the variadic mode is
    // passed as an unsigned int, as C's default promotion of `mode_t` does.
    owned(unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags, mode) })
}

fn mkdir_at(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<()> {
    // SAFETY: `dir` is an open descriptor and `name` a NUL-terminated
    // string, both alive for the call, which only reads `name`.
    let result = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn symlink_at(target: &CStr, dir: BorrowedFd<'_>, name: &CStr) -> io::Result<()> {
    // SAFETY: `target` and `name` are NUL-terminated strings and `dir` an
    // open descriptor, all alive for the call, which only reads them.
    let result = unsafe { libc::symlinkat(target.as_ptr(), dir.as_raw_fd(), name.as_ptr()) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
        /// Any entry type, with data.
        Raw(&'a str, EntryType, &'a [u8]),
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
                    Item::Raw(path, kind, data) => (path, kind, 0o755, data, None),
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
            max_metadata_bytes: 1024,
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
            max_metadata_bytes: 1024,
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

    /// Everything under `dir` but `skip`, as paths relative to `dir`.
    fn everything_in(dir: &Path, skip: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in fs::read_dir(&at).unwrap() {
                let path = entry.unwrap().path();
                if path == skip {
                    continue;
                }
                found.push(path.strip_prefix(dir).unwrap().to_path_buf());
                if fs::symlink_metadata(&path).unwrap().is_dir() {
                    stack.push(path);
                }
            }
        }
        found
    }

    #[test]
    fn case_aliases_and_chained_links_write_nothing_outside_staging() {
        // `up` reaches the staging dir's parent through `out` and `here`.
        // On a case-insensitive volume `UP` is the same name, which a check
        // of paths alone misses. Nothing may land outside, whatever the
        // order of the entries.
        let links = [
            Item::Symlink("Localify.app/here", "."),
            Item::Symlink("Localify.app/out", "here/.."),
            Item::Symlink("Localify.app/up", "out/.."),
        ];
        for payload in ["Localify.app/UP/payload", "Localify.app/Up/x/payload"] {
            for links_first in [true, false] {
                let file = Item::File(payload, b"escaped", 0o644);
                let mut items = vec![Item::Dir("Localify.app/")];
                if links_first {
                    items.extend(links.iter().map(clone_item));
                    items.push(file);
                } else {
                    items.push(file);
                    items.extend(links.iter().map(clone_item));
                }
                let outer = tempfile::tempdir().unwrap();
                let staging = outer.path().join("staging");
                fs::create_dir(&staging).unwrap();
                let result = extract_bundle(&archive(&items), &staging, LIMITS);
                assert!(result.is_err(), "{payload} was accepted");
                assert_eq!(
                    everything_in(outer.path(), &staging),
                    Vec::<PathBuf>::new(),
                    "{payload}, links first: {links_first}"
                );
            }
        }
    }

    fn clone_item<'a>(item: &Item<'a>) -> Item<'a> {
        match *item {
            Item::Symlink(path, to) => Item::Symlink(path, to),
            _ => unreachable!("only symlinks are cloned"),
        }
    }

    #[test]
    fn an_entry_under_a_symlink_listed_after_it_is_refused() {
        let bytes = archive(&[
            Item::File("Localify.app/Contents/Link/file", b"x", 0o644),
            Item::Symlink("Localify.app/Contents/Link", "."),
        ]);
        assert!(matches!(extract(&bytes).1, Err(ExtractError::Duplicate(_))));
    }

    /// A path longer than a tar header's 100 bytes.
    fn long_path(leaf: &str) -> String {
        format!("Localify.app/Contents/{}/{leaf}", "d".repeat(120))
    }

    /// An archive written by `tar`'s own builder, which puts long names in
    /// GNU long-name headers, and PAX records before entries when asked.
    fn built(write: impl FnOnce(&mut tar::Builder<GzEncoder<Vec<u8>>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        write(&mut builder);
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn file_header(size: usize) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(size as u64);
        header
    }

    #[test]
    fn long_names_from_gnu_and_pax_headers_are_used() {
        let gnu = long_path("gnu");
        let pax = long_path("pax");
        let bytes = built(|builder| {
            builder
                .append_data(&mut file_header(1), &gnu, &b"g"[..])
                .unwrap();
            builder
                .append_pax_extensions([("path", pax.as_bytes()), ("mtime", b"1.5".as_slice())])
                .unwrap();
            let mut header = file_header(1);
            header.set_path("Localify.app/short").unwrap();
            header.set_cksum();
            builder.append(&header, &b"p"[..]).unwrap();
        });
        let (dir, result) = extract(&bytes);
        result.unwrap();
        assert_eq!(fs::read(dir.path().join(&gnu)).unwrap(), b"g");
        assert_eq!(fs::read(dir.path().join(&pax)).unwrap(), b"p");
        assert!(!dir.path().join("Localify.app/short").exists());
    }

    #[test]
    fn long_name_and_pax_headers_count_toward_the_entry_cap() {
        // Two files, four headers: tar alone would show two entries.
        let bytes = built(|builder| {
            for leaf in ["a", "b"] {
                builder
                    .append_data(&mut file_header(1), long_path(leaf), &b"x"[..])
                    .unwrap();
            }
        });
        let limits = Limits {
            max_entries: 3,
            ..LIMITS
        };
        assert!(matches!(
            extract_with(&bytes, limits).1,
            Err(ExtractError::TooManyEntries)
        ));
    }

    #[test]
    fn oversized_metadata_is_refused_before_it_is_read() {
        let huge = vec![b'a'; 2 * 1024 * 1024];
        let pax = built(|builder| {
            builder
                .append_pax_extensions([("comment", huge.as_slice())])
                .unwrap();
            builder
                .append_data(&mut file_header(1), "Localify.app/a", &b"x"[..])
                .unwrap();
        });
        let name = format!("Localify.app/{}", "n".repeat(2 * 1024 * 1024));
        let gnu = built(|builder| {
            builder
                .append_data(&mut file_header(1), &name, &b"x"[..])
                .unwrap();
        });
        for bytes in [pax, gnu] {
            assert!(matches!(
                extract(&bytes).1,
                Err(ExtractError::MetadataTooBig)
            ));
        }
    }

    #[test]
    fn pax_records_that_move_entries_and_global_headers_are_refused() {
        for key in ["size", "GNU.sparse.size"] {
            let bytes = built(|builder| {
                builder
                    .append_pax_extensions([(key, b"0".as_slice())])
                    .unwrap();
                builder
                    .append_data(&mut file_header(1), "Localify.app/a", &b"x"[..])
                    .unwrap();
            });
            assert!(
                matches!(extract(&bytes).1, Err(ExtractError::Metadata(_))),
                "{key} was accepted"
            );
        }
        let global = archive(&[Item::Raw(
            "pax_global_header",
            EntryType::XGlobalHeader,
            b"11 a=bcdef\n",
        )]);
        assert!(matches!(extract(&global).1, Err(ExtractError::Metadata(_))));
    }

    #[test]
    fn payloads_on_entries_that_are_not_files_are_refused() {
        let big = vec![0u8; 64 * 1024];
        for kind in [EntryType::Directory, EntryType::Symlink] {
            let bytes = archive(&[Item::Raw("Localify.app/x", kind, &big)]);
            let limits = Limits {
                max_bytes: 1024,
                ..LIMITS
            };
            assert!(
                matches!(
                    extract_with(&bytes, limits).1,
                    Err(ExtractError::Payload(_))
                ),
                "{kind:?} with a payload was accepted"
            );
        }
    }

    #[test]
    fn the_decompressed_stream_stops_at_its_cap() {
        let mut capped = Capped::new(&b"abcdef"[..], 4);
        assert!(capped.read_to_end(&mut Vec::new()).is_err());
        assert!(capped.exceeded);
        let mut exact = Capped::new(&b"abcd"[..], 4);
        let mut read = Vec::new();
        exact.read_to_end(&mut read).unwrap();
        assert_eq!((read.as_slice(), exact.exceeded), (&b"abcd"[..], false));
    }
}
