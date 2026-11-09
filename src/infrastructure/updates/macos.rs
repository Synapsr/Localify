//! What the installer asks of macOS: where the bundle is and whether this
//! account may replace it, its signature, the processes running from it,
//! and the atomic swap.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use super::rules::{ProcessPath, parse_version};
use crate::branding::{BRANDING, user_home_dir};

/// Localify's Apple team, read from the shipped app's signature
/// (`codesign -dvv /Applications/Localify.app`: `TeamIdentifier=P5G8WAJ2G2`,
/// `Authority=Developer ID Application: Téo Goulois (P5G8WAJ2G2)`).
pub const TEAM_ID: &str = "P5G8WAJ2G2";
/// `identifier` in `src-tauri/tauri.conf.json`.
pub const BUNDLE_ID: &str = "com.localify.desktop";
/// The signing identifier of the bundle's `localify`, the daemon's
/// executable.
pub const CLI_ID: &str = "localify";

/// The bundle's `localify` binary, relative to the bundle.
pub const CLI_IN_BUNDLE: &str = "Contents/MacOS/localify";

/// Why the installer can't replace this copy of Localify. Each message is
/// what Settings and `localify update --status` show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    #[error("This build of Localify is a development build: update it by hand.")]
    DevBuild,
    #[error("This copy of Localify is not in an app bundle: update it by hand.")]
    NotInBundle,
    #[error(
        "Localify runs from {0}, not /Applications or ~/Applications: move it there to update automatically."
    )]
    Location(String),
    #[error("{0} is not Localify's app bundle: update it by hand.")]
    NotLocalify(String),
    #[error("{0} belongs to another account: only its owner can update it.")]
    Owner(String),
    #[error("This account can't replace {0}: update it by hand, or give it write access.")]
    NotWritable(String),
    #[error("Could not read {0}: {1}")]
    Unreadable(String, String),
}

/// The installed bundle the installer may replace, as it was when checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallTarget {
    pub bundle: PathBuf,
    pub version: Version,
    dev: u64,
    ino: u64,
}

impl InstallTarget {
    /// The bundle holding this very binary, when it is the production app
    /// at a supported place, owned and replaceable by this account.
    pub fn current() -> Result<Self, TargetError> {
        if BRANDING.is_dev_profile() || cfg!(debug_assertions) {
            return Err(TargetError::DevBuild);
        }
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|error| TargetError::Unreadable("this binary".into(), error.to_string()))?;
        let home = user_home_dir().unwrap_or_default();
        let allowed = [
            PathBuf::from("/Applications/Localify.app"),
            home.join("Applications/Localify.app"),
        ];
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        Self::at(&exe, &allowed, uid)
    }

    /// The rules, apart from where they are read: `exe` is this binary's
    /// canonical path, `allowed` the bundle paths that may be replaced.
    pub fn at(exe: &Path, allowed: &[PathBuf], uid: u32) -> Result<Self, TargetError> {
        let bundle = bundle_of(exe).ok_or(TargetError::NotInBundle)?;
        let shown = bundle.display().to_string();
        if !allowed.iter().any(|path| path == &bundle) {
            return Err(TargetError::Location(shown));
        }
        let unreadable = |error: String| TargetError::Unreadable(shown.clone(), error);
        if info_value(&bundle, "CFBundleIdentifier").map_err(|e| unreadable(e.to_string()))?
            != BUNDLE_ID
        {
            return Err(TargetError::NotLocalify(shown));
        }
        let meta = std::fs::symlink_metadata(&bundle).map_err(|e| unreadable(e.to_string()))?;
        if meta.uid() != uid {
            return Err(TargetError::Owner(shown));
        }
        let parent = bundle.parent().unwrap_or(Path::new("/"));
        if !writable(&bundle) || !writable(parent) {
            return Err(TargetError::NotWritable(shown));
        }
        let version = bundle_version(&bundle).map_err(|e| unreadable(e.to_string()))?;
        Ok(Self {
            bundle,
            version,
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }

    /// Fails when the bundle is no longer the one checked: another inode or
    /// another version. The installer then leaves it as it is.
    pub fn still_unchanged(&self) -> Result<()> {
        let meta = std::fs::symlink_metadata(&self.bundle)
            .with_context(|| format!("{} is gone", self.bundle.display()))?;
        if (meta.dev(), meta.ino()) != (self.dev, self.ino) {
            bail!(
                "{} was replaced while the update was prepared, so it was left as it is.",
                self.bundle.display()
            );
        }
        let version = bundle_version(&self.bundle)?;
        if version != self.version {
            bail!(
                "{} changed to {version} while the update was prepared, so it was left as it is.",
                self.bundle.display()
            );
        }
        Ok(())
    }

    /// The device this bundle is on: staging must be on the same one.
    pub fn device(&self) -> u64 {
        self.dev
    }
}

/// The version of the app bundle this binary runs from, if it runs from one.
pub fn running_bundle_version() -> Option<Version> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()?;
    bundle_version(&bundle_of(&exe)?).ok()
}

/// `/x/Localify.app` for `/x/Localify.app/Contents/MacOS/localify`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let shaped = exe.file_name() == Some(OsStr::new("localify"))
        && macos.file_name() == Some(OsStr::new("MacOS"))
        && contents.file_name() == Some(OsStr::new("Contents"))
        && bundle.extension() == Some(OsStr::new("app"));
    shaped.then(|| bundle.to_path_buf())
}

fn writable(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a NUL-terminated string that outlives the call,
    // which only reads it. A read-only volume answers EROFS.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// A string from the bundle's `Info.plist`, XML or binary.
pub fn info_value(bundle: &Path, key: &str) -> Result<String> {
    let plist = bundle.join("Contents/Info.plist");
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .with_context(|| format!("Could not run plutil on {}", plist.display()))?;
    if !output.status.success() {
        bail!("{} has no {key}", plist.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `CFBundleShortVersionString`, the version the app shows.
pub fn bundle_version(bundle: &Path) -> Result<Version> {
    let value = info_value(bundle, "CFBundleShortVersionString")?;
    parse_version(&value)
        .ok_or_else(|| anyhow!("{} has an invalid version {value}", bundle.display()))
}

/// Checks a staged bundle's code signature. Behind a trait because a test
/// can't make a bundle Apple signed.
pub trait SignatureCheck {
    fn verify(&self, bundle: &Path) -> Result<()>;
}

/// `codesign --verify --deep --strict`, with a requirement anchored to Apple
/// and pinned to Localify's team and identifiers, on the bundle and on the
/// daemon's executable inside it.
pub struct Codesign;

impl SignatureCheck for Codesign {
    fn verify(&self, bundle: &Path) -> Result<()> {
        codesign(bundle, &["--deep"], BUNDLE_ID)?;
        codesign(&bundle.join(CLI_IN_BUNDLE), &[], CLI_ID)
    }
}

/// A Developer ID requirement for `identifier` from Localify's team.
pub fn requirement(identifier: &str) -> String {
    format!(
        "anchor apple generic and identifier \"{identifier}\" \
         and certificate leaf[subject.OU] = \"{TEAM_ID}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
}

fn codesign(path: &Path, extra: &[&str], identifier: &str) -> Result<()> {
    let output = Command::new("/usr/bin/codesign")
        .arg("--verify")
        .args(extra)
        .arg("--strict")
        .arg("--test-requirement")
        .arg(format!("={}", requirement(identifier)))
        .arg(path)
        .output()
        .context("Could not run codesign")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} is not signed by Localify: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

/// Every process's executable path, as `proc_pidpath` reads it.
pub fn process_paths() -> Result<Vec<(i32, ProcessPath)>> {
    list_pids()?
        .into_iter()
        .map(|pid| Ok((pid, process_path(pid))))
        .collect()
}

fn list_pids() -> Result<Vec<i32>> {
    // SAFETY: a null buffer of size 0 only asks for the current count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        bail!("Could not list processes: {}", io::Error::last_os_error());
    }
    let mut pids: Vec<libc::c_int> = vec![0; count as usize * 2 + 64];
    let size = libc::c_int::try_from(pids.len() * size_of::<libc::c_int>())?;
    // SAFETY: `pids` holds `size` bytes of c_ints, and the call writes at
    // most `size` bytes.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    if filled <= 0 {
        bail!("Could not list processes: {}", io::Error::last_os_error());
    }
    // A full buffer may have cut the list short: then nothing is certain.
    if filled as usize >= pids.len() {
        bail!("Could not list every process: the list kept growing");
    }
    pids.truncate(filled as usize);
    Ok(pids)
}

fn process_path(pid: i32) -> ProcessPath {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buffer` holds PROC_PIDPATHINFO_MAXSIZE bytes, the size passed,
    // and the call writes at most that many.
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length > 0 {
        buffer.truncate(length as usize);
        return ProcessPath::Path(PathBuf::from(OsStr::from_bytes(&buffer)));
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => ProcessPath::Gone,
        Some(libc::ENOENT) => ProcessPath::Deleted,
        _ => ProcessPath::Unreadable,
    }
}

/// `acl_t` in `<sys/acl.h>`: an ACL the library allocates.
type Acl = *mut libc::c_void;
/// `ACL_TYPE_EXTENDED` in `<sys/acl.h>`, the only ACL type macOS uses.
const ACL_TYPE_EXTENDED: libc::c_int = 0x0000_0100;

// The ACL calls of libSystem, which the libc crate doesn't declare for
// macOS. Signatures from `<sys/acl.h>`; `acl_type_t` is a C enum, an int.
unsafe extern "C" {
    fn acl_init(count: libc::c_int) -> Acl;
    fn acl_set_fd_np(fd: libc::c_int, acl: Acl, kind: libc::c_int) -> libc::c_int;
    fn acl_free(object: *mut libc::c_void) -> libc::c_int;
}

/// Removes every ACL entry of an open file or directory, as `chmod -N`
/// does, so only its owner and its mode decide who may write in it. An ACL
/// inherited from a parent could otherwise let another account into the
/// installer's private directories, whatever their mode says.
pub fn clear_acl(file: &File) -> io::Result<()> {
    // SAFETY: no preconditions; it returns a new empty ACL, or null.
    let acl = unsafe { acl_init(0) };
    if acl.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `file`'s descriptor is open for the call, and `acl` is the
    // valid ACL just made, which the call only reads.
    let result = unsafe { acl_set_fd_np(file.as_raw_fd(), acl, ACL_TYPE_EXTENDED) };
    let error = io::Error::last_os_error();
    // SAFETY: `acl` came from `acl_init`, is freed once, and is not used
    // after.
    unsafe { acl_free(acl) };
    match result {
        0 => Ok(()),
        // A volume without ACLs has none to clear.
        _ if error.raw_os_error() == Some(libc::ENOTSUP) => Ok(()),
        _ => Err(error),
    }
}

/// Exchanges `staged` and `installed` in one step with `renamex_np`, so
/// `installed` is never missing. Fails, touching nothing, on a volume
/// without `RENAME_SWAP`.
pub fn swap(staged: &Path, installed: &Path) -> Result<()> {
    let from = CString::new(staged.as_os_str().as_bytes())?;
    let to = CString::new(installed.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated strings that outlive the call, which
    // only reads them.
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOTSUP) | Some(libc::EINVAL) => bail!(
            "The volume of {} can't swap two folders in one step (RENAME_SWAP), so it was left as it is.",
            installed.display()
        ),
        _ => Err(anyhow!(error).context(format!(
            "Could not swap the new bundle in at {}",
            installed.display()
        ))),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;

    use super::*;

    /// A bundle shaped like Localify's, unsigned.
    pub(crate) fn fake_bundle(at: &Path, version: &str) {
        fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        fs::write(at.join(CLI_IN_BUNDLE), version).unwrap();
        fs::write(
            at.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
    }

    fn uid() -> u32 {
        // SAFETY: no preconditions.
        unsafe { libc::geteuid() }
    }

    #[test]
    fn the_production_bundle_at_an_allowed_place_is_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(dir.path()).unwrap();
        let bundle = dir.join("Localify.app");
        fake_bundle(&bundle, "1.6.0");
        let target = InstallTarget::at(
            &bundle.join(CLI_IN_BUNDLE),
            std::slice::from_ref(&bundle),
            uid(),
        )
        .unwrap();
        assert_eq!(target.bundle, bundle);
        assert_eq!(target.version, Version::new(1, 6, 0));
        target.still_unchanged().unwrap();
    }

    #[test]
    fn other_places_owners_and_bundles_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Localify.app");
        fake_bundle(&bundle, "1.6.0");
        let exe = bundle.join(CLI_IN_BUNDLE);
        let elsewhere = PathBuf::from("/Applications/Localify.app");
        assert!(matches!(
            InstallTarget::at(&exe, std::slice::from_ref(&elsewhere), uid()),
            Err(TargetError::Location(_))
        ));
        assert!(matches!(
            InstallTarget::at(&exe, std::slice::from_ref(&bundle), uid() + 1),
            Err(TargetError::Owner(_))
        ));
        assert!(matches!(
            InstallTarget::at(
                Path::new("/usr/local/bin/localify"),
                std::slice::from_ref(&bundle),
                uid()
            ),
            Err(TargetError::NotInBundle)
        ));
        let plist = bundle.join("Contents/Info.plist");
        let text = fs::read_to_string(&plist)
            .unwrap()
            .replace(BUNDLE_ID, "com.localify.dev");
        fs::write(&plist, text).unwrap();
        assert!(matches!(
            InstallTarget::at(&exe, &[bundle], uid()),
            Err(TargetError::NotLocalify(_))
        ));
    }

    #[test]
    fn a_bundle_that_changed_since_the_check_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Localify.app");
        fake_bundle(&bundle, "1.6.0");
        let target = InstallTarget::at(
            &bundle.join(CLI_IN_BUNDLE),
            std::slice::from_ref(&bundle),
            uid(),
        )
        .unwrap();
        fake_bundle(&bundle, "1.6.1");
        assert!(target.still_unchanged().is_err());

        fs::rename(&bundle, dir.path().join("Old.app")).unwrap();
        fake_bundle(&bundle, "1.6.0");
        assert!(target.still_unchanged().is_err());
    }

    #[test]
    fn swapping_exchanges_the_two_folders() {
        // Temp dirs are on the boot volume, APFS on every supported macOS.
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Localify.app");
        let staged = dir.path().join("staged/Localify.app");
        fake_bundle(&installed, "1.6.0");
        fake_bundle(&staged, "1.7.0");
        swap(&staged, &installed).unwrap();
        assert_eq!(bundle_version(&installed).unwrap(), Version::new(1, 7, 0));
        assert_eq!(bundle_version(&staged).unwrap(), Version::new(1, 6, 0));
    }

    #[test]
    fn swapping_with_a_missing_side_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("Localify.app");
        fake_bundle(&installed, "1.6.0");
        assert!(swap(&dir.path().join("missing.app"), &installed).is_err());
        assert_eq!(bundle_version(&installed).unwrap(), Version::new(1, 6, 0));
    }

    #[test]
    fn this_test_process_is_in_the_process_list() {
        let me = std::env::current_exe().unwrap();
        let me = fs::canonicalize(me).unwrap();
        let processes = process_paths().unwrap();
        let pid = std::process::id() as i32;
        let found = processes.iter().find(|(p, _)| *p == pid).unwrap();
        assert_eq!(found.1, ProcessPath::Path(me));
    }

    /// Whether `ls -le` lists ACL entries for `path`.
    pub(crate) fn has_acl(path: &Path) -> bool {
        let output = Command::new("/bin/ls")
            .arg("-lde")
            .arg(path)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).lines().count() > 1
    }

    #[test]
    fn clearing_an_acl_removes_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let status = Command::new("/bin/chmod")
            .args(["+a", "everyone allow add_file,delete_child,file_inherit"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(has_acl(dir.path()));
        clear_acl(&File::open(dir.path()).unwrap()).unwrap();
        assert!(!has_acl(dir.path()));
    }

    #[test]
    fn the_requirement_pins_the_team_and_identifier() {
        let text = requirement(BUNDLE_ID);
        assert!(text.starts_with("anchor apple generic"));
        assert!(text.contains(r#"identifier "com.localify.desktop""#));
        assert!(text.contains(r#"leaf[subject.OU] = "P5G8WAJ2G2""#));
    }

    #[test]
    fn an_unsigned_bundle_fails_codesign() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Localify.app");
        fake_bundle(&bundle, "1.7.0");
        assert!(Codesign.verify(&bundle).is_err());
    }
}
