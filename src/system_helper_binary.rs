//! The root-owned copy of the executable that launchd runs as the system helper.
//!
//! launchd runs the helper as root, and the helper starts the daemon from its
//! own executable. Both must come from a file no other user can change, so the
//! service never points at the installed app or CLI, which usually belong to
//! the user. Only an administrator-approved `helper ensure-running` writes this
//! copy; the helper never updates itself from a user-writable path.
//!
//! Root also must not trust a config another user could change, which is why
//! this module checks that location too.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{
    DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, chown, fchown,
};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;
use crate::infrastructure::privilege::running_as_root;

const LAUNCHD_PLIST_DIR: &str = "/Library/LaunchDaemons";
const PRIVILEGED_HELPER_DIR: &str = "/Library/PrivilegedHelperTools";
const INSTALLED_MODE: u32 = 0o755;
const WRITABLE_BY_OTHERS: u32 = 0o022;
const STICKY: u32 = 0o1000;

/// Who must own the copy and every directory above it. Always root in
/// production; tests pass their own user so they can run unprivileged.
#[derive(Debug, Clone, Copy)]
struct TrustedOwner {
    uid: u32,
    gid: u32,
}

const ROOT: TrustedOwner = TrustedOwner { uid: 0, gid: 0 };

/// Scoped by UID because each user of the Mac gets their own helper and socket.
pub fn helper_service_label(uid: u32) -> String {
    format!("{}.{uid}", BRANDING.helper_service_prefix())
}

/// launchd reads the service from this root-owned directory at boot.
pub fn helper_plist_path(uid: u32) -> PathBuf {
    PathBuf::from(LAUNCHD_PLIST_DIR).join(format!("{}.plist", helper_service_label(uid)))
}

/// The label names the copy, so a helper left by an older dev install is a
/// separate file from the one the production helper runs.
pub fn helper_binary_path(uid: u32) -> PathBuf {
    PathBuf::from(PRIVILEGED_HELPER_DIR).join(helper_service_label(uid))
}

/// Copies `source` over the helper's root-owned copy.
///
/// Call this only from a process an administrator approved: the bytes come
/// from whatever is at `source` now, and that approval is the only check on
/// them. Fails if the location could be changed by a non-root user.
pub fn install_helper_binary(source: &Path, uid: u32) -> Result<()> {
    install_copy(source, &helper_binary_path(uid), Path::new("/"), ROOT)
}

/// Refuses to install a debug build as the root helper. Only release builds
/// get one: the dev profile runs without a helper.
pub fn ensure_build_may_install_helper() -> Result<()> {
    check_build_may_install(cfg!(debug_assertions))
}

/// Stops any root process from acting on a config another user could change,
/// because that config steers root. Does nothing when this process is not root.
///
/// The file must be root's, not a link, and reachable only through directories
/// no other user can change. Fails closed with what to do about it.
pub fn ensure_root_may_use_config(path: &Path) -> Result<()> {
    if !running_as_root() {
        return Ok(());
    }

    if BRANDING.is_dev_profile() {
        bail!(
            "The dev profile runs as your user and never as root. Run {} without sudo.",
            BRANDING.cli_command()
        );
    }

    verify_config_location(path, Path::new("/"), ROOT).with_context(|| {
        format!(
            "Refusing to run as root with the config '{}', because another user could change it.\n\
             Use the default config, or run {} without sudo.",
            path.display(),
            BRANDING.cli_command()
        )
    })
}

/// Whether the service runs the root-owned copy. False for a plist written by
/// an older version, which runs the installed executable directly.
pub fn helper_service_uses_root_copy(uid: u32) -> Result<bool> {
    plist_runs_binary(&helper_plist_path(uid), &helper_binary_path(uid))
}

/// Whether an existing service is safe and already runs the bytes of
/// `executable`. Anything else needs `helper ensure-running` as administrator.
pub fn helper_service_is_current(uid: u32, executable: &Path) -> Result<bool> {
    service_is_current(
        &helper_plist_path(uid),
        &helper_binary_path(uid),
        executable,
        ROOT,
    )
}

/// Whether root can run `path` without any other user being able to change it.
pub fn binary_is_root_protected(path: &Path) -> bool {
    is_protected(path, Path::new("/"), ROOT)
}

/// UIDs with a helper service installed, for uninstalling them all.
pub fn installed_helper_uids() -> Result<Vec<u32>> {
    let entries = match fs::read_dir(LAUNCHD_PLIST_DIR) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to read {LAUNCHD_PLIST_DIR} to find helpers"));
        }
    };

    let mut uids = Vec::new();
    for entry in entries {
        let name = entry
            .with_context(|| format!("Failed to read {LAUNCHD_PLIST_DIR} to find helpers"))?
            .file_name();
        if let Some(uid) = name.to_str().and_then(uid_from_plist_name) {
            uids.push(uid);
        }
    }
    Ok(uids)
}

/// Deletes a file or link if it exists, without following a link.
pub fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Builds the LaunchDaemon plist. It takes no executable path so a caller
/// cannot point launchd at a user-writable file.
pub fn launchd_plist_contents(config_path: &Path, socket: &Path, uid: u32) -> String {
    // No environment: the helper never runs with a profile, and its service must
    // not be steerable by one.
    let args = [
        helper_binary_path(uid).to_string_lossy().to_string(),
        "--config".into(),
        config_path.to_string_lossy().to_string(),
        "helper".into(),
        "serve".into(),
        "--socket".into(),
        socket.to_string_lossy().to_string(),
        "--uid".into(),
        uid.to_string(),
    ];

    let program_arguments = args
        .iter()
        .map(|arg| format!("    <string>{}</string>", escape_xml(arg)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
{}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>1</integer>
</dict>
</plist>
"#,
        escape_xml(&helper_service_label(uid)),
        program_arguments,
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn check_build_may_install(debug_build: bool) -> Result<()> {
    if debug_build {
        bail!(
            "Refusing to install a debug build as the {} system helper.\n\
             Set {}=dev to run without a helper, or install a release build.",
            BRANDING.app_name,
            BRANDING.profile_env()
        );
    }
    Ok(())
}

fn uid_from_plist_name(name: &str) -> Option<u32> {
    name.strip_prefix(&format!("{}.", BRANDING.helper_service_prefix()))?
        .strip_suffix(".plist")?
        .parse()
        .ok()
}

fn service_is_current(
    plist_path: &Path,
    copy: &Path,
    executable: &Path,
    owner: TrustedOwner,
) -> Result<bool> {
    Ok(plist_runs_binary(plist_path, copy)?
        && verify_protected_file(copy, owner).is_ok()
        && same_contents(copy, executable)?)
}

fn plist_runs_binary(plist_path: &Path, binary: &Path) -> Result<bool> {
    let plist = match fs::read_to_string(plist_path) {
        Ok(plist) => plist,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to read the {} helper plist at {}",
                    BRANDING.app_name,
                    plist_path.display()
                )
            });
        }
    };

    Ok(plist_program(&plist) == Some(escape_xml(&binary.to_string_lossy()).as_str()))
}

/// The executable launchd starts: the first string of `ProgramArguments`.
fn plist_program(plist: &str) -> Option<&str> {
    let (_, arguments) = plist.split_once("<key>ProgramArguments</key>")?;
    let (_, first) = arguments.split_once("<string>")?;
    first.split_once("</string>").map(|(program, _)| program)
}

fn same_contents(copy: &Path, executable: &Path) -> Result<bool> {
    let read = |path: &Path| {
        fs::read(path).with_context(|| format!("Failed to read '{}'", path.display()))
    };
    Ok(read(copy)? == read(executable)?)
}

fn is_protected(path: &Path, trust_root: &Path, owner: TrustedOwner) -> bool {
    let Ok(path) = fs::canonicalize(path) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    verify_protected_file(&path, owner).is_ok()
        && verify_directory_chain(parent, trust_root, owner).is_ok()
}

fn install_copy(
    source: &Path,
    destination: &Path,
    trust_root: &Path,
    owner: TrustedOwner,
) -> Result<()> {
    let (Some(parent), Some(name)) = (destination.parent(), destination.file_name()) else {
        bail!(
            "Helper binary path '{}' has no parent directory.",
            destination.display()
        );
    };
    prepare_parent(parent, trust_root, owner)?;
    reject_unsafe_destination(destination, owner)?;

    let temp = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let _ = remove_if_present(&temp);
    let result = write_private_copy(source, &temp, owner).and_then(|()| {
        fs::rename(&temp, destination).with_context(|| {
            format!(
                "Failed to move the helper binary into place at '{}'",
                destination.display()
            )
        })
    });
    if result.is_err() {
        let _ = remove_if_present(&temp);
    }
    result?;

    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("Failed to sync '{}'", parent.display()))?;
    verify_protected_file(destination, owner)
}

/// Creates the file only the owner can open, gives it to the trusted owner, and
/// makes it executable last, so it is never writable by another user.
fn write_private_copy(source: &Path, temp: &Path, owner: TrustedOwner) -> Result<()> {
    let mut input = File::open(source)
        .with_context(|| format!("Failed to open the executable '{}'", source.display()))?;
    if !input
        .metadata()
        .with_context(|| format!("Failed to inspect '{}'", source.display()))?
        .is_file()
    {
        bail!("Executable '{}' is not a regular file.", source.display());
    }

    let mut output = create_private_file(temp)
        .with_context(|| format!("Failed to create '{}'", temp.display()))?;
    io::copy(&mut input, &mut output)
        .with_context(|| format!("Failed to copy the executable to '{}'", temp.display()))?;
    fchown(&output, Some(owner.uid), Some(owner.gid)).with_context(|| {
        format!(
            "Failed to give '{}' to the system account. Run this as an administrator.",
            temp.display()
        )
    })?;
    output
        .set_permissions(Permissions::from_mode(INSTALLED_MODE))
        .with_context(|| format!("Failed to set permissions on '{}'", temp.display()))?;
    output
        .sync_all()
        .with_context(|| format!("Failed to sync '{}'", temp.display()))
}

/// `create_new` fails rather than follow anything already at this path.
fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
}

fn prepare_parent(parent: &Path, trust_root: &Path, owner: TrustedOwner) -> Result<()> {
    if let Some(above) = parent.parent() {
        verify_directory_chain(above, trust_root, owner)?;
    }

    match DirBuilder::new().mode(INSTALLED_MODE).create(parent) {
        Ok(()) => {
            chown(parent, Some(owner.uid), Some(owner.gid))
                .and_then(|()| fs::set_permissions(parent, Permissions::from_mode(INSTALLED_MODE)))
                .with_context(|| format!("Failed to secure '{}'", parent.display()))?;
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to create '{}'", parent.display()));
        }
    }

    verify_directory(parent, owner, false)
}

fn reject_unsafe_destination(destination: &Path, owner: TrustedOwner) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to inspect '{}'", destination.display()))
        }
        Ok(_) => verify_protected_file(destination, owner),
    }
}

fn verify_directory_chain(dir: &Path, trust_root: &Path, owner: TrustedOwner) -> Result<()> {
    for ancestor in dir
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(trust_root))
    {
        verify_directory(ancestor, owner, true)?;
    }
    Ok(())
}

/// Checks a config path as given, component by component, so a link anywhere on
/// the way counts. A link is acceptable only if root owns it, and then what it
/// points at must pass too: `/etc` is such a link on macOS.
fn verify_config_location(path: &Path, trust_root: &Path, owner: TrustedOwner) -> Result<()> {
    let path = std::path::absolute(path)
        .with_context(|| format!("Failed to resolve '{}'", path.display()))?;
    let exists = match fs::symlink_metadata(&path) {
        Ok(_) => {
            verify_protected_file(&path, owner)?;
            true
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to inspect '{}'", path.display()));
        }
    };

    let Some(parent) = path.parent() else {
        bail!("Config path '{}' has no parent directory.", path.display());
    };
    let Some(nearest) = parent
        .ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor).is_ok())
    else {
        bail!("Config path '{}' has no existing parent.", path.display());
    };

    // Another user could create a missing file in a sticky directory first.
    if !exists {
        verify_nearest_config_directory(nearest, owner)?;
    }
    for ancestor in nearest
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(trust_root))
    {
        verify_config_directory(ancestor, trust_root, owner)?;
    }
    Ok(())
}

fn verify_nearest_config_directory(path: &Path, owner: TrustedOwner) -> Result<()> {
    let target = fs::canonicalize(path)
        .with_context(|| format!("Failed to resolve '{}'", path.display()))?;
    verify_directory(&target, owner, false)
}

fn verify_config_directory(path: &Path, trust_root: &Path, owner: TrustedOwner) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect '{}'", path.display()))?;
    if !metadata.is_symlink() {
        return verify_directory(path, owner, true);
    }

    if metadata.uid() != owner.uid {
        bail!(
            "'{}' is a link belonging to UID {}, which could point it anywhere.",
            path.display(),
            metadata.uid()
        );
    }
    let target = fs::canonicalize(path)
        .with_context(|| format!("Failed to resolve '{}'", path.display()))?;
    verify_directory_chain(&target, trust_root, owner)
}

/// `allow_sticky` covers directories above the copy: in a sticky directory,
/// other users cannot rename or replace an entry they do not own.
fn verify_directory(path: &Path, owner: TrustedOwner, allow_sticky: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect '{}'", path.display()))?;
    if !metadata.is_dir() {
        bail!(
            "'{}' must be a real directory, not a link. Remove it as an administrator and try again.",
            path.display()
        );
    }
    if metadata.uid() != owner.uid {
        bail!(
            "'{}' must belong to the system account (it belongs to UID {}). \
             Fix its owner as an administrator and try again.",
            path.display(),
            metadata.uid()
        );
    }
    let mode = metadata.mode();
    if mode & WRITABLE_BY_OTHERS != 0 && !(allow_sticky && mode & STICKY != 0) {
        bail!(
            "'{}' can be changed by other users (mode {:o}). \
             Fix its permissions as an administrator and try again.",
            path.display(),
            mode & 0o7777
        );
    }
    Ok(())
}

fn verify_protected_file(path: &Path, owner: TrustedOwner) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect '{}'", path.display()))?;
    if !metadata.is_file() {
        bail!(
            "'{}' must be a regular file, not a link. Remove it as an administrator and try again.",
            path.display()
        );
    }
    if metadata.uid() != owner.uid {
        bail!(
            "'{}' must belong to the system account (it belongs to UID {}). \
             Remove it as an administrator and try again.",
            path.display(),
            metadata.uid()
        );
    }
    if metadata.mode() & WRITABLE_BY_OTHERS != 0 {
        bail!(
            "'{}' can be changed by other users (mode {:o}). \
             Remove it as an administrator and try again.",
            path.display(),
            metadata.mode() & 0o7777
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Layout {
        // Keeps the directory alive for the whole test.
        _temp: tempfile::TempDir,
        root: PathBuf,
        owner: TrustedOwner,
        source: PathBuf,
        destination: PathBuf,
    }

    impl Layout {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            fs::set_permissions(&root, Permissions::from_mode(0o755)).unwrap();
            let metadata = fs::metadata(&root).unwrap();
            let owner = TrustedOwner {
                uid: metadata.uid(),
                gid: metadata.gid(),
            };
            let source = root.join("localify");
            fs::write(&source, b"new build").unwrap();
            let destination = root.join("helpers").join("dev.localify.helper.501");
            Self {
                _temp: temp,
                root,
                owner,
                source,
                destination,
            }
        }

        fn install(&self) -> Result<()> {
            install_copy(&self.source, &self.destination, &self.root, self.owner)
        }

        fn parent(&self) -> PathBuf {
            self.destination.parent().unwrap().to_path_buf()
        }

        fn entries_in_parent(&self) -> Vec<String> {
            fs::read_dir(self.parent())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        }

        fn another_user(&self) -> TrustedOwner {
            TrustedOwner {
                uid: self.owner.uid + 1,
                gid: self.owner.gid,
            }
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn helper_service_label_is_uid_scoped() {
        assert_eq!(helper_service_label(501), "dev.localify.helper.501");
    }

    #[test]
    fn helper_binary_lives_in_the_root_owned_directory() {
        assert_eq!(
            helper_binary_path(501),
            Path::new("/Library/PrivilegedHelperTools/dev.localify.helper.501")
        );
    }

    #[test]
    fn plist_runs_the_root_owned_copy_and_never_the_current_executable() {
        let plist = launchd_plist_contents(
            Path::new("/etc/localify/config.toml"),
            Path::new("/var/run/localify-helper-501.sock"),
            501,
        );
        let current = std::env::current_exe().unwrap();

        assert_eq!(
            plist_program(&plist),
            Some("/Library/PrivilegedHelperTools/dev.localify.helper.501")
        );
        assert!(!plist.contains(current.to_str().unwrap()));
    }

    #[test]
    fn plist_contains_expected_program_arguments() {
        let plist = launchd_plist_contents(
            Path::new("/etc/localify/config.toml"),
            Path::new("/var/run/localify-helper-501.sock"),
            501,
        );

        assert!(plist.contains("<string>dev.localify.helper.501</string>"));
        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/etc/localify/config.toml</string>"));
        assert!(plist.contains("<string>helper</string>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>--socket</string>"));
        assert!(plist.contains("<string>/var/run/localify-helper-501.sock</string>"));
        assert!(plist.contains("<string>--uid</string>"));
        assert!(plist.contains("<string>501</string>"));
        assert!(plist.contains("<key>ThrottleInterval</key>"));
    }

    #[test]
    fn plist_escapes_xml_in_arguments() {
        let plist = launchd_plist_contents(
            Path::new("/etc/a&b/config.toml"),
            Path::new("/var/run/h.sock"),
            501,
        );

        assert!(plist.contains("<string>/etc/a&amp;b/config.toml</string>"));
    }

    #[test]
    fn plist_from_an_older_version_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("copy");
        let plist_path = dir.path().join("helper.plist");
        fs::write(
            &plist_path,
            format!(
                "<key>ProgramArguments</key><array>\
                 <string>/Applications/Localify.app/Contents/MacOS/localify</string>\
                 <string>--config</string><string>{}</string></array>",
                copy.display()
            ),
        )
        .unwrap();

        assert!(!plist_runs_binary(&plist_path, &copy).unwrap());
    }

    #[test]
    fn plist_with_the_copy_as_program_is_current() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("a&b");
        let plist_path = dir.path().join("helper.plist");
        fs::write(
            &plist_path,
            format!(
                "<key>ProgramArguments</key><array><string>{}</string></array>",
                escape_xml(&copy.to_string_lossy())
            ),
        )
        .unwrap();

        assert!(plist_runs_binary(&plist_path, &copy).unwrap());
    }

    #[test]
    fn missing_plist_does_not_run_the_copy() {
        let dir = tempfile::tempdir().unwrap();

        assert!(
            !plist_runs_binary(&dir.path().join("missing.plist"), &dir.path().join("copy"))
                .unwrap()
        );
    }

    #[test]
    fn install_copies_the_bytes_with_the_expected_owner_and_mode() {
        let layout = Layout::new();

        layout.install().unwrap();

        let metadata = fs::symlink_metadata(&layout.destination).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.uid(), layout.owner.uid);
        assert_eq!(metadata.gid(), layout.owner.gid);
        assert_eq!(metadata.mode() & 0o7777, 0o755);
        assert_eq!(fs::read(&layout.destination).unwrap(), b"new build");
    }

    #[test]
    fn install_creates_a_missing_parent_with_a_safe_mode() {
        let layout = Layout::new();

        layout.install().unwrap();

        let parent = fs::symlink_metadata(layout.parent()).unwrap();
        assert_eq!(parent.mode() & 0o7777, 0o755);
        assert_eq!(parent.uid(), layout.owner.uid);
    }

    #[test]
    fn install_replaces_the_file_atomically_with_a_new_inode() {
        let layout = Layout::new();
        layout.install().unwrap();
        let before = fs::metadata(&layout.destination).unwrap().ino();
        let running = File::open(&layout.destination).unwrap();

        fs::write(&layout.source, b"newer build").unwrap();
        layout.install().unwrap();

        // A process already running the old file keeps its bytes, and readers
        // of the path only ever see a complete file.
        assert_ne!(fs::metadata(&layout.destination).unwrap().ino(), before);
        let mut old_bytes = Vec::new();
        io::Read::read_to_end(&mut &running, &mut old_bytes).unwrap();
        assert_eq!(old_bytes, b"new build");
        assert_eq!(fs::read(&layout.destination).unwrap(), b"newer build");
    }

    #[test]
    fn install_leaves_no_temporary_file() {
        let layout = Layout::new();

        layout.install().unwrap();

        assert_eq!(
            layout.entries_in_parent(),
            vec!["dev.localify.helper.501".to_string()]
        );
    }

    #[test]
    fn the_temporary_file_starts_private_and_ends_executable() {
        let layout = Layout::new();
        let temp = layout.root.join("temp");

        let file = create_private_file(&temp).unwrap();
        assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o700);
        drop(file);
        remove_if_present(&temp).unwrap();

        write_private_copy(&layout.source, &temp, layout.owner).unwrap();
        assert_eq!(fs::metadata(&temp).unwrap().mode() & 0o7777, 0o755);
    }

    #[test]
    fn write_private_copy_does_not_follow_an_existing_link() {
        let layout = Layout::new();
        let victim = layout.root.join("victim");
        fs::write(&victim, b"victim").unwrap();
        let temp = layout.root.join("temp");
        symlink(&victim, &temp).unwrap();

        assert!(write_private_copy(&layout.source, &temp, layout.owner).is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"victim");
    }

    #[test]
    fn install_rejects_a_symlinked_destination_without_touching_its_target() {
        let layout = Layout::new();
        fs::create_dir(layout.parent()).unwrap();
        let victim = layout.root.join("victim");
        fs::write(&victim, b"victim").unwrap();
        symlink(&victim, &layout.destination).unwrap();

        let error = layout.install().unwrap_err();

        assert!(error.to_string().contains("regular file"));
        assert_eq!(fs::read(&victim).unwrap(), b"victim");
        assert!(
            fs::symlink_metadata(&layout.destination)
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn install_rejects_a_user_writable_destination() {
        let layout = Layout::new();
        fs::create_dir(layout.parent()).unwrap();
        fs::write(&layout.destination, b"planted").unwrap();
        set_mode(&layout.destination, 0o777);

        let error = layout.install().unwrap_err();

        assert!(error.to_string().contains("changed by other users"));
        assert_eq!(fs::read(&layout.destination).unwrap(), b"planted");
    }

    #[test]
    fn install_rejects_a_destination_owned_by_another_user() {
        let layout = Layout::new();
        layout.install().unwrap();

        let error = reject_unsafe_destination(&layout.destination, layout.another_user());

        assert!(error.unwrap_err().to_string().contains("system account"));
    }

    #[test]
    fn install_rejects_a_writable_parent() {
        let layout = Layout::new();
        fs::create_dir(layout.parent()).unwrap();

        for mode in [0o775, 0o777, 0o1777] {
            set_mode(&layout.parent(), mode);
            let error = layout.install().unwrap_err();
            assert!(error.to_string().contains("changed by other users"));
        }
        assert!(!layout.destination.exists());
    }

    #[test]
    fn install_rejects_a_symlinked_parent() {
        let layout = Layout::new();
        let elsewhere = layout.root.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, layout.parent()).unwrap();

        let error = layout.install().unwrap_err();

        assert!(error.to_string().contains("real directory"));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn install_rejects_a_parent_owned_by_another_user() {
        let layout = Layout::new();
        fs::create_dir(layout.parent()).unwrap();

        let error = verify_directory(&layout.parent(), layout.another_user(), false);

        assert!(error.unwrap_err().to_string().contains("system account"));
    }

    #[test]
    fn install_rejects_a_writable_directory_above_the_parent() {
        let layout = Layout::new();
        set_mode(&layout.root, 0o777);

        let error = layout.install().unwrap_err();

        assert!(error.to_string().contains("changed by other users"));
        assert!(!layout.parent().exists());
    }

    #[test]
    fn a_sticky_directory_above_the_parent_is_accepted() {
        let layout = Layout::new();
        set_mode(&layout.root, 0o1775);

        layout.install().unwrap();

        assert!(layout.destination.exists());
    }

    #[test]
    fn a_failed_copy_keeps_the_previous_binary_and_leaves_no_temporary_file() {
        let layout = Layout::new();
        layout.install().unwrap();
        fs::remove_file(&layout.source).unwrap();

        assert!(layout.install().is_err());

        assert_eq!(fs::read(&layout.destination).unwrap(), b"new build");
        assert_eq!(
            layout.entries_in_parent(),
            vec!["dev.localify.helper.501".to_string()]
        );
    }

    #[test]
    fn install_rejects_a_source_that_is_not_a_file() {
        let layout = Layout::new();
        fs::remove_file(&layout.source).unwrap();
        fs::create_dir(&layout.source).unwrap();

        let error = layout.install().unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
    }

    fn current_plist(layout: &Layout) -> PathBuf {
        let plist_path = layout.root.join("helper.plist");
        fs::write(
            &plist_path,
            format!(
                "<key>ProgramArguments</key><array><string>{}</string></array>",
                layout.destination.display()
            ),
        )
        .unwrap();
        plist_path
    }

    #[test]
    fn service_is_current_when_it_runs_the_same_bytes_from_a_protected_copy() {
        let layout = Layout::new();
        layout.install().unwrap();
        let plist = current_plist(&layout);

        assert!(
            service_is_current(&plist, &layout.destination, &layout.source, layout.owner).unwrap()
        );
    }

    #[test]
    fn an_app_update_makes_the_service_stale() {
        let layout = Layout::new();
        layout.install().unwrap();
        let plist = current_plist(&layout);

        // Same length, different bytes: only a content comparison notices.
        fs::write(&layout.source, b"NEW BUILD").unwrap();
        assert!(
            !service_is_current(&plist, &layout.destination, &layout.source, layout.owner).unwrap()
        );

        fs::write(&layout.source, b"a longer build").unwrap();
        assert!(
            !service_is_current(&plist, &layout.destination, &layout.source, layout.owner).unwrap()
        );
    }

    #[test]
    fn service_with_an_unprotected_copy_is_not_current() {
        let layout = Layout::new();
        layout.install().unwrap();
        let plist = current_plist(&layout);

        assert!(
            !service_is_current(
                &plist,
                &layout.destination,
                &layout.source,
                layout.another_user()
            )
            .unwrap()
        );

        set_mode(&layout.destination, 0o775);
        assert!(
            !service_is_current(&plist, &layout.destination, &layout.source, layout.owner).unwrap()
        );
    }

    #[test]
    fn service_is_not_current_when_the_copy_is_missing() {
        let layout = Layout::new();
        let plist = current_plist(&layout);

        assert!(
            !service_is_current(&plist, &layout.destination, &layout.source, layout.owner).unwrap()
        );
    }

    #[test]
    fn a_missing_executable_is_an_error_and_not_a_silent_match() {
        let layout = Layout::new();
        layout.install().unwrap();
        let plist = current_plist(&layout);
        fs::remove_file(&layout.source).unwrap();

        assert!(
            service_is_current(&plist, &layout.destination, &layout.source, layout.owner).is_err()
        );
    }

    #[test]
    fn a_binary_is_protected_only_when_it_and_its_directories_are() {
        let layout = Layout::new();
        layout.install().unwrap();
        assert!(is_protected(
            &layout.destination,
            &layout.root,
            layout.owner
        ));

        // Through a link it resolves to the same protected file.
        let link = layout.root.join("link");
        symlink(&layout.destination, &link).unwrap();
        assert!(is_protected(&link, &layout.root, layout.owner));

        assert!(!is_protected(
            &layout.destination,
            &layout.root,
            layout.another_user()
        ));

        set_mode(&layout.parent(), 0o777);
        assert!(!is_protected(
            &layout.destination,
            &layout.root,
            layout.owner
        ));
        set_mode(&layout.parent(), 0o755);

        set_mode(&layout.destination, 0o777);
        assert!(!is_protected(
            &layout.destination,
            &layout.root,
            layout.owner
        ));

        assert!(!is_protected(
            &layout.root.join("missing"),
            &layout.root,
            layout.owner
        ));
    }

    #[test]
    fn a_file_in_a_temp_directory_is_not_root_protected() {
        let layout = Layout::new();
        set_mode(&layout.source, 0o666);

        assert!(!binary_is_root_protected(&layout.source));
    }

    #[test]
    fn debug_builds_never_install_a_root_helper() {
        assert!(check_build_may_install(true).is_err());
        assert!(check_build_may_install(false).is_ok());
    }

    fn config_layout() -> (Layout, PathBuf) {
        let layout = Layout::new();
        let directory = layout.root.join("etc");
        fs::create_dir(&directory).unwrap();
        set_mode(&directory, 0o755);
        let config = directory.join("config.toml");
        fs::write(&config, "").unwrap();
        set_mode(&config, 0o644);
        (layout, config)
    }

    #[test]
    fn a_protected_config_is_accepted() {
        let (layout, config) = config_layout();

        verify_config_location(&config, &layout.root, layout.owner).unwrap();
    }

    #[test]
    fn a_symlinked_config_is_refused() {
        let (layout, target) = config_layout();
        let config = target.parent().unwrap().join("link.toml");
        symlink(&target, &config).unwrap();

        let error = verify_config_location(&config, &layout.root, layout.owner).unwrap_err();

        assert!(error.to_string().contains("not a link"));
    }

    #[test]
    fn a_config_owned_by_another_user_is_refused() {
        let (layout, config) = config_layout();

        let error =
            verify_config_location(&config, &layout.root, layout.another_user()).unwrap_err();

        assert!(error.to_string().contains("system account"));
    }

    #[test]
    fn a_config_other_users_can_write_is_refused() {
        let (layout, config) = config_layout();
        set_mode(&config, 0o664);

        let error = verify_config_location(&config, &layout.root, layout.owner).unwrap_err();

        assert!(error.to_string().contains("changed by other users"));
    }

    #[test]
    fn a_config_under_a_writable_directory_is_refused() {
        let (layout, config) = config_layout();
        set_mode(config.parent().unwrap(), 0o777);

        let error = verify_config_location(&config, &layout.root, layout.owner).unwrap_err();

        assert!(error.to_string().contains("changed by other users"));
    }

    #[test]
    fn a_config_under_a_directory_of_another_user_is_refused() {
        let (layout, config) = config_layout();

        let error = verify_config_directory(
            config.parent().unwrap(),
            &layout.root,
            layout.another_user(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("system account"));
    }

    #[test]
    fn a_config_through_a_link_of_another_user_is_refused() {
        let (layout, config) = config_layout();
        let link = layout.root.join("linked-etc");
        symlink(config.parent().unwrap(), &link).unwrap();

        let error =
            verify_config_directory(&link, &layout.root, layout.another_user()).unwrap_err();

        assert!(error.to_string().contains("a link belonging"));
    }

    #[test]
    fn a_config_through_a_trusted_link_is_checked_at_its_target() {
        let (layout, config) = config_layout();
        let link = layout.root.join("linked-etc");
        symlink(config.parent().unwrap(), &link).unwrap();
        let through_link = link.join("config.toml");

        verify_config_location(&through_link, &layout.root, layout.owner).unwrap();

        set_mode(config.parent().unwrap(), 0o777);
        assert!(verify_config_location(&through_link, &layout.root, layout.owner).is_err());
    }

    #[test]
    fn a_missing_config_is_accepted_only_in_a_protected_directory() {
        let (layout, config) = config_layout();
        let missing = config.parent().unwrap().join("nested/missing.toml");

        verify_config_location(&missing, &layout.root, layout.owner).unwrap();

        // In a sticky directory another user could create the file first.
        set_mode(config.parent().unwrap(), 0o1777);
        assert!(verify_config_location(&missing, &layout.root, layout.owner).is_err());
    }

    #[test]
    fn a_root_owned_config_in_a_sticky_directory_is_accepted() {
        let (layout, config) = config_layout();
        set_mode(config.parent().unwrap(), 0o1777);

        verify_config_location(&config, &layout.root, layout.owner).unwrap();
    }

    #[test]
    fn this_process_may_use_any_config_when_it_is_not_root() {
        if running_as_root() {
            return;
        }

        ensure_root_may_use_config(Path::new("/tmp/any/config.toml")).unwrap();
    }

    #[test]
    fn uids_come_from_this_profiles_plist_names() {
        assert_eq!(
            uid_from_plist_name("dev.localify.helper.501.plist"),
            Some(501)
        );
        assert_eq!(uid_from_plist_name("dev.localify.helper.abc.plist"), None);
        assert_eq!(
            uid_from_plist_name("dev.localify-dev.helper.501.plist"),
            None
        );
        assert_eq!(uid_from_plist_name("com.example.other.plist"), None);
    }

    #[test]
    fn remove_if_present_tolerates_a_missing_file_and_does_not_follow_links() {
        let layout = Layout::new();
        let victim = layout.root.join("victim");
        fs::write(&victim, b"victim").unwrap();
        let link = layout.root.join("link");
        symlink(&victim, &link).unwrap();

        remove_if_present(&link).unwrap();
        remove_if_present(&link).unwrap();

        assert!(!link.exists());
        assert_eq!(fs::read(&victim).unwrap(), b"victim");
    }
}
