//! Localify's one installer (docs/desktop.md, "Automatic updates"): the
//! daemon runs it unattended, and `localify update install` when a person
//! asks. Every rule of the contract is enforced here, in this order:
//! signature, extraction, version, code signature, then, under the update
//! lock, the app checks, the destination check, the staged bundle checked
//! again, and the swap.
//!
//! The staging directory is held open from its creation to the swap, which
//! names both sides from open directories. Every folder above it must be
//! one no other account can change.

use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use super::UpdatePaths;
use super::extract::{self, ExtractError, LIMITS};
use super::macos::{self, InstallTarget, SignatureCheck};
use super::release::{self, Release};
use super::rules::{self, ProcessPath, parse_version};
use super::state::StateStore;

/// Who asked for the install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The daemon: only with `automatic` on just before replacing, and
    /// only while the app is closed.
    Unattended,
    /// A person ran `localify update install`: waits up to this long for
    /// the app to quit.
    Confirmed { wait_for_app: Duration },
}

/// What the installer depends on, so tests can supply their own.
pub struct Installer<'a> {
    pub paths: &'a UpdatePaths,
    pub target: InstallTarget,
    pub public_key: &'a str,
    pub signature: &'a dyn SignatureCheck,
    pub processes: &'a dyn Fn() -> Result<Vec<(i32, ProcessPath)>>,
    /// Reads `[updates] automatic` from the config, at the last moment.
    pub automatic: &'a dyn Fn() -> bool,
}

/// A bundle ready to swap in, in the installer's private directory, which
/// is removed when this is dropped.
pub struct Staged {
    dir: StagingDir,
    bundle: PathBuf,
    version: Version,
}

/// What an install did.
#[derive(Debug)]
pub struct Installed {
    pub version: Version,
    pub previous: Previous,
}

/// Where the bundle an install replaced is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    /// In `previous/`, the documented recovery copy.
    Kept(PathBuf),
    /// Moving it there failed, so it stayed in its staging directory.
    Stranded { at: PathBuf, error: String },
}

/// Why staging failed: the archive's fault, which is remembered so the
/// release is skipped, or this machine's, such as a full disk.
enum Refusal {
    Archive(anyhow::Error),
    Here(anyhow::Error),
}

/// The staged bundle's path led somewhere else than the directory held
/// open, so what a tool checked through it isn't what would be swapped in.
#[derive(Debug, thiserror::Error)]
#[error("{0} no longer leads to the staged update, so nothing was replaced.")]
struct StagingMoved(String);

/// [`extract::BUNDLE_NAME`], for the calls that name it from a directory.
const BUNDLE: &CStr = c"Localify.app";

/// What a release that failed here before answers.
pub fn failed_before(version: &str) -> anyhow::Error {
    anyhow!("Localify {version} failed to install here before. Waiting for the next release.")
}

impl Installer<'_> {
    /// Whether `release` may replace the installed bundle at all: strictly
    /// newer than it and than every version installed here.
    pub fn wants(&self, release: &Release) -> Result<Option<Version>> {
        let offered = parse_version(&release.version)
            .ok_or_else(|| anyhow!("The update manifest has an invalid version"))?;
        let highest = StateStore::new(self.paths).load()?.highest();
        let newer = rules::offer_is_newer(&offered, &self.target.version, highest.as_ref());
        Ok(newer.then_some(offered))
    }

    /// Whether this release's archive failed here before, known from the
    /// manifest alone, so it isn't downloaded again.
    pub fn rejected(&self, release: &Release) -> Result<bool> {
        let state = StateStore::new(self.paths).load()?;
        Ok(state.was_rejected(&release.identity()))
    }

    /// Checks the archive and expands it into a new private directory: the
    /// signature before anything is extracted, then the bundle's version
    /// and its code signature. An archive that fails its signature, the
    /// extraction rules, or the version check is recorded, by its SHA-256 and
    /// by the release's identity, and skipped until another release. A
    /// `plutil` or `codesign` failure is only retried at the next check,
    /// since those tools also fail for local reasons.
    pub fn stage(&self, release: &Release, archive: &[u8]) -> Result<Staged> {
        let digest = release::sha256_hex(archive);
        let identity = release.identity();
        let store = StateStore::new(self.paths);
        let state = store.load()?;
        if state.has_failed(&digest) || state.was_rejected(&identity) {
            store.update(self.paths, |state| state.record_rejected(&identity))?;
            return Err(failed_before(&release.version));
        }
        match self.verify_and_expand(release, archive) {
            Ok(staged) => Ok(staged),
            Err(Refusal::Here(error)) => Err(error),
            Err(Refusal::Archive(error)) => {
                self.log(&format!("rejected sha256={digest}: {error:#}"));
                store.update(self.paths, |state| {
                    state.record_failed(&digest);
                    state.record_rejected(&identity);
                })?;
                Err(error)
            }
        }
    }

    fn verify_and_expand(&self, release: &Release, archive: &[u8]) -> Result<Staged, Refusal> {
        let manifest_archive = release.archive.as_ref().ok_or_else(|| {
            Refusal::Here(anyhow!("The update manifest has no archive for this Mac"))
        })?;
        release::verify_signature(archive, &manifest_archive.signature, self.public_key)
            .map_err(Refusal::Archive)?;
        let dir = StagingDir::create(self.paths, self.target.device()).map_err(Refusal::Here)?;
        let bundle = match extract::extract_bundle(archive, &dir.dir, dir.path(), LIMITS) {
            Ok(bundle) => bundle,
            Err(error @ ExtractError::Write(_)) => {
                return Err(Refusal::Here(
                    anyhow!(error).context("Could not stage the update"),
                ));
            }
            Err(error) => {
                return Err(Refusal::Archive(
                    anyhow!(error).context("The update archive was refused"),
                ));
            }
        };
        // The archive passed its minisign signature, so a tool's "no" here is
        // retried at the next check rather than held against the release:
        // `plutil` and `codesign` also fail for local reasons (too many open
        // files, a damaged keychain) with the same exit codes.
        let version = dir
            .through_path(macos::bundle_version)
            .map_err(Refusal::Here)?;
        if version.to_string() != release.version {
            return Err(Refusal::Archive(anyhow!(
                "The update archive holds version {version}, not the {} the manifest names",
                release.version
            )));
        }
        dir.through_path(|bundle| self.signature.verify(bundle))
            .map_err(Refusal::Here)?;
        Ok(Staged {
            dir,
            bundle,
            version,
        })
    }

    /// Swaps the staged bundle in, under the update lock, once the app is
    /// closed, the destination is still the one checked, and the staged
    /// bundle is still the one verified. The old bundle is kept in
    /// `previous/`, or, when that fails, left in staging and reported.
    pub fn replace(&self, staged: Staged, mode: Mode) -> Result<Installed> {
        let _held = self.take_locks(mode)?;
        let bundle_use = rules::bundle_use(&self.target.bundle, &(self.processes)()?);
        rules::app_is_closed(true, bundle_use)?;
        if mode == Mode::Unattended && !(self.automatic)() {
            bail!("Automatic updates were turned off, so nothing was replaced.");
        }
        self.target.still_unchanged()?;
        self.still_verified(&staged)?;
        self.swap(&staged)?;
        let version = staged.version.clone();
        self.log(&format!(
            "installed version={version} previous={} at={}",
            self.target.version,
            self.target.bundle.display()
        ));
        let previous = self.keep_previous(staged);
        let stranded = match &previous {
            Previous::Kept(_) => None,
            Previous::Stranded { at, .. } => Some(at.clone()),
        };
        StateStore::new(self.paths)
            .update(self.paths, |state| {
                state.saw_installed(&version);
                state.install_error = None;
                state.last_download_at = None;
                state.stranded_previous = stranded;
            })
            .with_context(|| format!("Installed {version}, but could not record it"))?;
        Ok(Installed { version, previous })
    }

    /// The staged bundle as verified, checked again right before the swap:
    /// the same version, and its code signature, of the bundle in the
    /// directory held open.
    fn still_verified(&self, staged: &Staged) -> Result<()> {
        let changed = "The staged update changed after it was verified, so nothing was replaced";
        let version = staged
            .dir
            .through_path(macos::bundle_version)
            .context(changed)?;
        if version != staged.version {
            bail!("{changed}: it is now version {version}.");
        }
        staged
            .dir
            .through_path(|bundle| self.signature.verify(bundle))
            .context(changed)
    }

    /// Swaps the bundle in the staging directory held open with the
    /// installed one, named from its folder.
    fn swap(&self, staged: &Staged) -> Result<()> {
        let installed = &self.target.bundle;
        let parent = installed.parent().unwrap_or(Path::new("/"));
        let name = installed
            .file_name()
            .ok_or_else(|| anyhow!("{} has no name", installed.display()))?;
        let name = CString::new(name.as_bytes())?;
        let destination = open_dir(parent)?;
        macos::swap_at(
            staged.dir.dir.as_fd(),
            BUNDLE,
            destination.as_fd(),
            &name,
            installed,
        )
    }

    /// The update lock, then `app.lock`. A person's install waits for the
    /// app to quit without holding the update lock meanwhile, so an app
    /// starting then is never stuck behind it.
    fn take_locks(&self, mode: Mode) -> Result<(File, File)> {
        let wait = match mode {
            Mode::Unattended => Duration::ZERO,
            Mode::Confirmed { wait_for_app } => wait_for_app,
        };
        let started = Instant::now();
        loop {
            let update = open_lock(&self.paths.update_lock())?;
            match mode {
                Mode::Unattended => update
                    .try_lock()
                    .map_err(|_| anyhow!("Another install is running."))?,
                Mode::Confirmed { .. } => update.lock()?,
            }
            let app = open_lock(&self.paths.app_lock())?;
            if app.try_lock().is_ok() {
                return Ok((update, app));
            }
            drop(update);
            if started.elapsed() >= wait {
                return Err(rules::AppRunning::LockHeld.into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Keeps the bundle swapped out, which sits where the staged one was.
    /// When it can't go to `previous/`, its staging directory is kept.
    fn keep_previous(&self, staged: Staged) -> Previous {
        let Staged { dir, bundle, .. } = staged;
        match self.move_to_previous(&bundle) {
            Ok(kept) => {
                self.log(&format!("kept previous={}", kept.display()));
                Previous::Kept(kept)
            }
            Err(error) => {
                dir.keep();
                self.log(&format!(
                    "could not keep the previous bundle in previous/: {error}; it stays at {}",
                    bundle.display()
                ));
                Previous::Stranded {
                    at: bundle,
                    error: error.to_string(),
                }
            }
        }
    }

    /// Moves `old_bundle` into `previous/`, and only then removes the copy
    /// kept before, so a failure never leaves no copy at all. It ends as
    /// `previous/<version>.app`, or under its temporary name when that last
    /// rename fails.
    fn move_to_previous(&self, old_bundle: &Path) -> io::Result<PathBuf> {
        let previous = self.paths.previous_dir();
        fs::create_dir_all(&previous)?;
        let incoming = previous.join(format!(".incoming-{}", super::random_u64()));
        fs::rename(old_bundle, &incoming)?;
        let kept = previous.join(format!("{}.app", self.target.version));
        let settled = remove_dir_contents_except(&previous, &incoming)
            .and_then(|()| fs::rename(&incoming, &kept));
        match settled {
            Ok(()) => Ok(kept),
            Err(error) => {
                self.log(&format!("an older previous bundle stays: {error}"));
                Ok(incoming)
            }
        }
    }

    pub fn log(&self, message: &str) {
        super::log(self.paths, message);
    }
}

/// Whether the daemon downloads `release` now: an update this Mac wants,
/// that didn't fail here before, with the app closed, and no unattended
/// download within the cooldown. The download is recorded before it starts,
/// so neither a restart nor a paired machine's hint repeats it.
pub fn may_download_unattended(
    installer: &Installer<'_>,
    release: &Release,
    now: u64,
) -> Result<bool> {
    if installer.wants(release)?.is_none() || installer.rejected(release)? {
        return Ok(false);
    }
    app_is_closed_now(installer)?;
    StateStore::new(installer.paths).update(installer.paths, |state| state.start_download(now))
}

/// The app checks without the update lock, before a download the daemon
/// would waste while the app is open. They run again under the lock.
pub fn app_is_closed_now(installer: &Installer<'_>) -> Result<()> {
    let app = open_lock(&installer.paths.app_lock())?;
    let free = app.try_lock().is_ok();
    drop(app);
    let bundle_use = rules::bundle_use(&installer.target.bundle, &(installer.processes)()?);
    rules::app_is_closed(free, bundle_use)?;
    Ok(())
}

fn open_lock(path: &Path) -> Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Could not open {}", path.display()))
}

fn remove_dir_contents_except(dir: &Path, keep: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path == keep {
            continue;
        }
        if fs::symlink_metadata(&path)?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// A new private directory under the updates folder, on the destination's
/// volume, held open from its creation to the swap, and removed with what
/// it holds when dropped unless kept.
struct StagingDir {
    path: PathBuf,
    dir: File,
    keep: bool,
}

impl StagingDir {
    /// Only this account may change it, or any folder above it. Every
    /// folder from `/` down to the updates folder must be one no other
    /// account can change (see [`check_above_staging`]). The updates folder,
    /// `staging/` and the new directory are owned by this account, writable
    /// by nobody else by their mode, and have no ACL, inherited or not. Each
    /// is opened from its parent, never through a symlink. The new
    /// directory is checked empty once that holds.
    fn create(paths: &UpdatePaths, device: u64) -> Result<Self> {
        fs::create_dir_all(paths.dir())
            .with_context(|| format!("Could not create {}", paths.dir().display()))?;
        // The path the tools are given is the one whose every folder was
        // checked: a symlink in the configured spelling would let a folder
        // above it that no one checked redirect the tools elsewhere.
        let (updates, checked) = open_checked(paths.dir())?;
        make_private(&updates, &checked, 0o755)?;
        let root = checked.join("staging");
        let staging_root = subdir(&updates, c"staging", &root, true)?;
        make_private(&staging_root, &root, 0o700)?;
        let name = format!("{}-{}", std::process::id(), super::random_u64());
        let path = root.join(&name);
        let dir = subdir(&staging_root, &CString::new(name)?, &path, false)?;
        let staging = Self {
            path,
            dir,
            keep: false,
        };
        make_private(&staging.dir, &staging.path, 0o700)?;
        if fs::read_dir(&staging.path)?.next().is_some() {
            bail!(
                "Something else wrote in {}, so the update wasn't staged there.",
                staging.path.display()
            );
        }
        if staging.dir.metadata()?.dev() != device {
            bail!(
                "{} is on another volume than the installed app, so the update can't be swapped in.",
                staging.path.display()
            );
        }
        Ok(staging)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `check` on the staged bundle through its path, which `codesign`
    /// and `plutil` need, and makes sure right before and right after that
    /// the path leads to the bundle in the directory held open.
    fn through_path<T>(&self, check: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
        let bundle = self.path.join(extract::BUNDLE_NAME);
        self.leads_to_held(&bundle)?;
        let checked = check(&bundle);
        self.leads_to_held(&bundle)?;
        checked
    }

    fn leads_to_held(&self, bundle: &Path) -> Result<()> {
        let moved = || StagingMoved(bundle.display().to_string());
        let held = extract::open_dir_at(self.dir.as_fd(), BUNDLE)
            .and_then(|fd| File::from(fd).metadata())
            .map_err(|_| moved())?;
        let seen = fs::symlink_metadata(bundle).map_err(|_| moved())?;
        if (held.dev(), held.ino()) != (seen.dev(), seen.ino()) {
            return Err(moved().into());
        }
        Ok(())
    }

    /// Leaves the directory and what it holds in place.
    fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Opens a directory, never through a symlink.
fn open_dir(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("Could not open {}", path.display()))
}

/// Opens the folder at `path` from `/`, one component at a time and never
/// through a symlink, once every folder above it passed
/// [`check_above_staging`].
fn open_checked(path: &Path) -> Result<(File, PathBuf)> {
    let canonical =
        fs::canonicalize(path).with_context(|| format!("Could not open {}", path.display()))?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let mut dir = open_dir(Path::new("/"))?;
    let mut at = PathBuf::from("/");
    for component in canonical.components().skip(1) {
        let Component::Normal(name) = component else {
            bail!("Could not open {}: not a plain path", canonical.display());
        };
        check_above_staging(&dir, &at, uid)?;
        at.push(name);
        let opened = extract::open_dir_at(dir.as_fd(), &CString::new(name.as_bytes())?);
        dir = File::from(opened.with_context(|| format!("Could not open {}", at.display()))?);
    }
    Ok((dir, canonical))
}

/// Refuses a folder above staging that another account could change: it
/// could rename what lies below and put its own tree at the staged
/// bundle's path. It must be owned by this account or root, have no group
/// or other write in its mode, and no ACL entry letting another account
/// write in it.
fn check_above_staging(dir: &File, path: &Path, uid: u32) -> Result<()> {
    let meta = dir
        .metadata()
        .with_context(|| format!("Could not read {}", path.display()))?;
    let acl = || {
        macos::acl_lets_others_write(dir, uid)
            .with_context(|| format!("Could not read the ACL of {}", path.display()))
    };
    let why = if meta.uid() != uid && meta.uid() != 0 {
        "belongs to another account"
    } else if meta.mode() & 0o022 != 0 {
        "is writable by its group or by everyone"
    } else if acl()? {
        "has an ACL entry that lets another account write in it"
    } else {
        return Ok(());
    };
    bail!(
        "{} {why}, so another account could swap the update for its own before it is installed. \
         Updates install again once only you or the system can write in it.",
        path.display()
    )
}

/// Makes (when missing, if `existing` may be) and opens the directory
/// `name` in `parent`, never through a symlink.
fn subdir(parent: &File, name: &CStr, shown: &Path, existing: bool) -> Result<File> {
    match extract::mkdir_at(parent.as_fd(), name) {
        Err(error) if !(existing && error.raw_os_error() == Some(libc::EEXIST)) => {
            return Err(error).with_context(|| format!("Could not create {}", shown.display()));
        }
        _ => {}
    }
    let opened = extract::open_dir_at(parent.as_fd(), name)
        .with_context(|| format!("Could not open {}", shown.display()))?;
    Ok(File::from(opened))
}

/// Checks that this account owns the open directory at `path`, then lets
/// only the owner write in it: its mode keeps at most `keep` (and always
/// the owner's access), and its ACL is cleared.
fn make_private(dir: &File, path: &Path, keep: u32) -> Result<()> {
    let meta = dir.metadata()?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if meta.uid() != unsafe { libc::geteuid() } {
        bail!(
            "{} belongs to another account, so the update can't be staged there.",
            path.display()
        );
    }
    let mode = (meta.mode() & keep) | 0o700;
    let private = || -> io::Result<()> {
        dir.set_permissions(Permissions::from_mode(mode))?;
        macos::clear_acl(dir)
    };
    private().with_context(|| format!("Could not make {} private", path.display()))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    #[test]
    fn the_tools_are_given_the_checked_path_not_a_symlinked_spelling() {
        // A symlink in the configured spelling would let a folder above it,
        // which nothing checked, point the tools at another tree.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let (_, checked) = open_checked(&link).unwrap();

        assert_eq!(checked, fs::canonicalize(&real).unwrap());
        assert!(
            !fs::symlink_metadata(&checked)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    use super::*;
    use crate::infrastructure::updates::extract::tests::with_write_error;
    use crate::infrastructure::updates::extract::tests::{Item, archive};
    use crate::infrastructure::updates::macos::tests::{add_acl, fake_bundle, has_acl, user_name};
    use crate::infrastructure::updates::macos::{CLI_IN_BUNDLE, TargetError};
    use crate::infrastructure::updates::release::ReleaseArchive;
    use crate::infrastructure::updates::release::tests::TestKey;

    struct Signed(bool);

    impl SignatureCheck for Signed {
        fn verify(&self, _bundle: &Path) -> Result<()> {
            if self.0 {
                Ok(())
            } else {
                bail!("not signed by Localify")
            }
        }
    }

    struct World {
        _dir: tempfile::TempDir,
        root: PathBuf,
        paths: UpdatePaths,
        key: TestKey,
        public: String,
    }

    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(dir.path()).unwrap();
            fake_bundle(&root.join("Localify.app"), "1.6.0");
            let key = TestKey::new(3);
            Self {
                paths: UpdatePaths::new(root.join("Support")),
                _dir: dir,
                root,
                public: key.public(),
                key,
            }
        }

        fn bundle(&self) -> PathBuf {
            self.root.join("Localify.app")
        }

        fn target(&self) -> Result<InstallTarget, TargetError> {
            // SAFETY: no preconditions.
            let uid = unsafe { libc::geteuid() };
            InstallTarget::at(&self.bundle().join(CLI_IN_BUNDLE), &[self.bundle()], uid)
        }

        fn installer<'a>(
            &'a self,
            signature: &'a dyn SignatureCheck,
            processes: &'a dyn Fn() -> Result<Vec<(i32, ProcessPath)>>,
            automatic: &'a dyn Fn() -> bool,
        ) -> Installer<'a> {
            Installer {
                paths: &self.paths,
                target: self.target().unwrap(),
                public_key: &self.public,
                signature,
                processes,
                automatic,
            }
        }

        /// A release whose archive holds a bundle of `bundle_version`.
        fn release(&self, version: &str, bundle_version: &str) -> (Release, Vec<u8>) {
            let plist = format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                 <key>CFBundleIdentifier</key><string>com.localify.desktop</string>\
                 <key>CFBundleShortVersionString</key><string>{bundle_version}</string>\
                 </dict></plist>"
            );
            let bytes = archive(&[
                Item::Dir("Localify.app/"),
                Item::File("Localify.app/Contents/Info.plist", plist.as_bytes(), 0o644),
                Item::File("Localify.app/Contents/MacOS/localify", b"new", 0o755),
            ]);
            let release = Release {
                version: version.into(),
                archive: Some(ReleaseArchive {
                    url: "https://d.test/a.tar.gz".into(),
                    signature: self.key.sign(&bytes),
                }),
            };
            (release, bytes)
        }
    }

    fn none() -> Result<Vec<(i32, ProcessPath)>> {
        Ok(vec![(1, ProcessPath::Path("/sbin/launchd".into()))])
    }

    #[test]
    fn a_signed_newer_release_is_swapped_in_and_the_old_one_kept() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert_eq!(
            installer.wants(&release).unwrap(),
            Some(Version::new(1, 7, 0))
        );

        // A copy kept by an earlier install.
        fake_bundle(&world.paths.previous_dir().join("1.5.0.app"), "1.5.0");

        let staged = installer.stage(&release, &bytes).unwrap();
        let installed = installer.replace(staged, Mode::Unattended).unwrap();

        assert_eq!(installed.version, Version::new(1, 7, 0));
        assert_eq!(
            fs::read(world.bundle().join(CLI_IN_BUNDLE)).unwrap(),
            b"new"
        );
        let kept = world.paths.previous_dir().join("1.6.0.app");
        assert_eq!(installed.previous, Previous::Kept(kept.clone()));
        assert_eq!(macos::bundle_version(&kept).unwrap(), Version::new(1, 6, 0));
        assert_eq!(fs::read_dir(world.paths.previous_dir()).unwrap().count(), 1);
        assert_eq!(fs::read_dir(world.paths.staging_root()).unwrap().count(), 0);
        let state = StateStore::new(&world.paths).load().unwrap();
        assert_eq!(state.highest(), Some(Version::new(1, 7, 0)));
        assert_eq!(state.stranded_previous, None);
        // The same release is no longer an update.
        let after = world.installer(&Signed(true), &none, &|| true);
        assert_eq!(after.wants(&release).unwrap(), None);
    }

    #[test]
    fn a_bad_signature_is_refused_before_extraction_and_remembered() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (mut release, bytes) = world.release("1.7.0", "1.7.0");
        release.archive.as_mut().unwrap().signature = TestKey::new(9).sign(&bytes);

        let error = installer.stage(&release, &bytes).err().unwrap();
        assert!(error.to_string().contains("signature"), "{error:#}");
        assert!(!world.paths.staging_root().exists());
        let digest = release::sha256_hex(&bytes);
        assert!(
            StateStore::new(&world.paths)
                .load()
                .unwrap()
                .has_failed(&digest)
        );

        // Even correctly signed later, the same archive is skipped.
        let (good, _) = world.release("1.7.0", "1.7.0");
        let again = installer.stage(&good, &bytes).err().unwrap();
        assert!(again.to_string().contains("failed to install here before"));
    }

    #[test]
    fn a_bundle_whose_version_differs_from_the_manifest_is_refused() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.6.5");
        let error = installer.stage(&release, &bytes).err().unwrap();
        assert!(error.to_string().contains("1.6.5"), "{error:#}");
    }

    #[test]
    fn a_bundle_codesign_refuses_is_not_installed_but_retried_later() {
        // codesign also exits 1 for local reasons (too many open files), so
        // its "no" is never held against a release whose minisign signature
        // passed.
        let world = World::new();
        let installer = world.installer(&Signed(false), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert!(installer.stage(&release, &bytes).is_err());
        let digest = release::sha256_hex(&bytes);
        assert!(
            !StateStore::new(&world.paths)
                .load()
                .unwrap()
                .has_failed(&digest)
        );
        assert!(!installer.rejected(&release).unwrap());
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
    }

    #[test]
    fn an_older_or_equal_release_or_one_below_the_highest_is_not_wanted() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        assert_eq!(
            installer.wants(&world.release("1.6.0", "1.6.0").0).unwrap(),
            None
        );
        assert_eq!(
            installer.wants(&world.release("1.5.0", "1.5.0").0).unwrap(),
            None
        );
        StateStore::new(&world.paths)
            .update(&world.paths, |state| {
                state.saw_installed(&Version::new(1, 8, 0))
            })
            .unwrap();
        assert_eq!(
            installer.wants(&world.release("1.7.0", "1.7.0").0).unwrap(),
            None
        );
    }

    #[test]
    fn the_app_holding_its_lock_stops_the_install() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let app = open_lock(&world.paths.app_lock()).unwrap();
        app.lock().unwrap();

        let staged = installer.stage(&release, &bytes).unwrap();
        let error = installer.replace(staged, Mode::Unattended).unwrap_err();
        assert!(
            error.downcast_ref::<rules::AppRunning>().is_some(),
            "{error:#}"
        );
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
        // A person's install waits, then gives up the same way.
        let staged = installer.stage(&release, &bytes).unwrap();
        let wait = Mode::Confirmed {
            wait_for_app: Duration::from_millis(200),
        };
        assert!(installer.replace(staged, wait).is_err());
    }

    #[test]
    fn a_persons_install_proceeds_once_the_app_quits() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| false);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let app = open_lock(&world.paths.app_lock()).unwrap();
        app.lock().unwrap();
        let quitting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(app);
        });

        let staged = installer.stage(&release, &bytes).unwrap();
        let wait = Mode::Confirmed {
            wait_for_app: Duration::from_secs(10),
        };
        // Confirmed installs don't need `automatic`.
        assert_eq!(
            installer.replace(staged, wait).unwrap().version,
            Version::new(1, 7, 0)
        );
        quitting.join().unwrap();
    }

    #[test]
    fn a_process_running_from_the_bundle_or_an_unknown_one_stops_the_install() {
        let world = World::new();
        let app_path = world.bundle().join("Contents/MacOS/localify-desktop");
        let running = move || Ok(vec![(42, ProcessPath::Path(app_path.clone()))]);
        let unknown = || Ok(vec![(43, ProcessPath::Unreadable)]);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        for processes in [
            &running as &dyn Fn() -> Result<Vec<(i32, ProcessPath)>>,
            &unknown,
        ] {
            let installer = world.installer(&Signed(true), processes, &|| true);
            let staged = installer.stage(&release, &bytes).unwrap();
            assert!(installer.replace(staged, Mode::Unattended).is_err());
        }
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
    }

    #[test]
    fn automatic_turned_off_at_the_last_moment_stops_an_unattended_install() {
        let world = World::new();
        let reads = Cell::new(0);
        let automatic = || {
            reads.set(reads.get() + 1);
            false
        };
        let installer = world.installer(&Signed(true), &none, &automatic);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let staged = installer.stage(&release, &bytes).unwrap();
        assert!(installer.replace(staged, Mode::Unattended).is_err());
        assert_eq!(reads.get(), 1);
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
    }

    /// Stages the live release with every real check: the built-in key,
    /// extraction, the version and `codesign`. Swaps nothing. Run with
    /// `LOCALIFY_UPDATE_MANIFEST=https://downloads.localify.dev/latest.json
    /// cargo test the_live_release -- --ignored`.
    #[test]
    #[ignore = "downloads the live release"]
    fn the_live_release_stages_with_every_real_check() {
        let endpoint = std::env::var("LOCALIFY_UPDATE_MANIFEST").unwrap();
        let release = release::fetch_manifest(&endpoint).unwrap();
        let world = World::new();
        let installer = Installer {
            public_key: release::updater_public_key(),
            ..world.installer(&macos::Codesign, &none, &|| true)
        };
        let url = &release.archive.as_ref().unwrap().url;
        let (bytes, digest) = release::download_archive(url).unwrap();
        let staged = installer.stage(&release, &bytes).unwrap();
        assert_eq!(staged.version.to_string(), release.version);
        println!("staged {} sha256={digest}", release.version);
    }

    #[test]
    fn a_destination_replaced_during_the_install_is_left_alone() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let staged = installer.stage(&release, &bytes).unwrap();
        // Someone drags 1.6.1 in from a DMG meanwhile.
        fs::rename(world.bundle(), world.root.join("Trash.app")).unwrap();
        fake_bundle(&world.bundle(), "1.6.1");

        assert!(installer.replace(staged, Mode::Unattended).is_err());
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 1)
        );
    }

    /// Verifies the first time only, as if the staged bundle changed after.
    struct SignedOnce(Cell<u32>);

    impl SignatureCheck for SignedOnce {
        fn verify(&self, _bundle: &Path) -> Result<()> {
            self.0.set(self.0.get() + 1);
            if self.0.get() == 1 {
                Ok(())
            } else {
                bail!("a sealed resource is missing or invalid")
            }
        }
    }

    #[test]
    fn a_staged_bundle_that_changed_after_verification_is_not_swapped_in() {
        let world = World::new();
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let once = SignedOnce(Cell::new(0));
        let installer = world.installer(&once, &none, &|| true);
        let staged = installer.stage(&release, &bytes).unwrap();
        assert!(installer.replace(staged, Mode::Unattended).is_err());

        let installer = world.installer(&Signed(true), &none, &|| true);
        let staged = installer.stage(&release, &bytes).unwrap();
        let plist = staged.bundle.join("Contents/Info.plist");
        let text = fs::read_to_string(&plist)
            .unwrap()
            .replace("1.7.0", "1.9.0");
        fs::write(&plist, text).unwrap();
        assert!(installer.replace(staged, Mode::Unattended).is_err());

        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
        // Not the archive's fault: the release isn't remembered as failed.
        assert!(!installer.rejected(&release).unwrap());
    }

    #[test]
    fn an_inherited_acl_never_reaches_the_staged_bundle() {
        let world = World::new();
        // New folders below let anyone add files.
        add_acl(
            &world.root,
            "everyone allow add_file,add_subdirectory,delete_child,file_inherit,directory_inherit,only_inherit",
        );
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");

        let staged = installer.stage(&release, &bytes).unwrap();
        for path in [
            world.paths.dir(),
            &world.paths.staging_root(),
            staged.dir.path(),
            &staged.bundle,
            &staged.bundle.join("Contents/Info.plist"),
        ] {
            assert!(!has_acl(path), "{} has an ACL", path.display());
        }
    }

    /// Whether this release is remembered as failed, by either record.
    fn remembered(
        world: &World,
        installer: &Installer<'_>,
        release: &Release,
        bytes: &[u8],
    ) -> bool {
        let state = StateStore::new(&world.paths).load().unwrap();
        installer.rejected(release).unwrap() || state.has_failed(&release::sha256_hex(bytes))
    }

    #[test]
    fn a_folder_above_staging_that_others_can_change_stops_the_install() {
        let others_write = |world: &World| {
            fs::set_permissions(&world.root, Permissions::from_mode(0o777)).unwrap();
        };
        let acl = |world: &World| add_acl(&world.root, "everyone allow delete_child");
        for open_up in [&others_write as &dyn Fn(&World), &acl] {
            let world = World::new();
            open_up(&world);
            let installer = world.installer(&Signed(true), &none, &|| true);
            let (release, bytes) = world.release("1.7.0", "1.7.0");

            let error = installer.stage(&release, &bytes).err().unwrap();
            let shown = world.root.display().to_string();
            assert!(error.to_string().starts_with(&shown), "{error:#}");
            assert!(!world.paths.staging_root().exists());
            // Not the archive's fault.
            assert!(!remembered(&world, &installer, &release, &bytes));
        }
    }

    #[test]
    fn a_folder_above_staging_only_this_account_can_change_is_fine() {
        let world = World::new();
        add_acl(&world.root, &format!("{} allow delete_child", user_name()));
        add_acl(&world.root, "everyone deny delete");
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let staged = installer.stage(&release, &bytes).unwrap();
        installer.replace(staged, Mode::Unattended).unwrap();
    }

    /// Passes, and meanwhile, on the check right before the swap, moves the
    /// staging directory away and puts another tree at its path, as
    /// another account renaming a folder above it would.
    struct SubstitutedDuringLastCheck(Cell<u32>);

    impl SignatureCheck for SubstitutedDuringLastCheck {
        fn verify(&self, bundle: &Path) -> Result<()> {
            self.0.set(self.0.get() + 1);
            if self.0.get() == 2 {
                let staging = bundle.parent().unwrap();
                fs::rename(staging, staging.with_extension("moved")).unwrap();
                fake_bundle(bundle, "6.6.6");
            }
            Ok(())
        }
    }

    #[test]
    fn a_tree_put_at_the_staging_path_is_never_swapped_in() {
        let world = World::new();
        let check = SubstitutedDuringLastCheck(Cell::new(0));
        let installer = world.installer(&check, &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let staged = installer.stage(&release, &bytes).unwrap();

        let error = installer.replace(staged, Mode::Unattended).unwrap_err();
        assert!(
            error.chain().any(|cause| cause.is::<StagingMoved>()),
            "{error:#}"
        );
        assert_eq!(
            macos::bundle_version(&world.bundle()).unwrap(),
            Version::new(1, 6, 0)
        );
    }

    #[test]
    fn a_full_disk_while_staging_is_not_the_archives_fault() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");

        let error = with_write_error(libc::ENOSPC, || installer.stage(&release, &bytes))
            .err()
            .unwrap();
        assert!(error.to_string().contains("Could not stage"), "{error:#}");
        assert!(!remembered(&world, &installer, &release, &bytes));
        // Once there is room, the same release installs.
        let staged = installer.stage(&release, &bytes).unwrap();
        installer.replace(staged, Mode::Unattended).unwrap();
    }

    /// The real `codesign` check, with no `codesign` to run.
    struct NoCodesign;

    impl SignatureCheck for NoCodesign {
        fn verify(&self, bundle: &Path) -> Result<()> {
            macos::verify_with(Path::new("/nonexistent/codesign"), bundle)
        }
    }

    #[test]
    fn a_codesign_that_cant_run_is_not_the_archives_fault() {
        let world = World::new();
        let installer = world.installer(&NoCodesign, &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert!(installer.stage(&release, &bytes).is_err());
        assert!(!remembered(&world, &installer, &release, &bytes));
    }

    #[test]
    fn the_bundle_name_is_the_archives() {
        assert_eq!(BUNDLE.to_str(), Ok(extract::BUNDLE_NAME));
    }

    #[test]
    fn a_previous_version_that_cant_be_kept_stays_in_staging_and_is_reported() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        // `previous/` can't be made: a file is in its way.
        fs::create_dir_all(world.paths.dir()).unwrap();
        fs::write(world.paths.previous_dir(), b"").unwrap();

        let staged = installer.stage(&release, &bytes).unwrap();
        let installed = installer.replace(staged, Mode::Unattended).unwrap();

        let Previous::Stranded { at, .. } = &installed.previous else {
            panic!("reported as kept: {:?}", installed.previous);
        };
        assert_eq!(macos::bundle_version(at).unwrap(), Version::new(1, 6, 0));
        assert!(at.starts_with(world.paths.staging_root()));
        let state = StateStore::new(&world.paths).load().unwrap();
        assert_eq!(state.stranded_previous.as_ref(), Some(at));
        let status = crate::infrastructure::updates::describe(
            &installed.version,
            &state,
            true,
            None,
            crate::infrastructure::updates::Activity::Idle,
        );
        assert!(status.reason.unwrap().contains(&at.display().to_string()));
    }

    #[test]
    fn the_older_copy_stays_until_the_new_one_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        let previous = world.paths.previous_dir();
        let older = previous.join("1.5.0.app");
        fake_bundle(&older, "1.5.0");
        // Nothing can be moved into `previous/`.
        fs::set_permissions(&previous, fs::Permissions::from_mode(0o500)).unwrap();

        let staged = installer.stage(&release, &bytes).unwrap();
        let installed = installer.replace(staged, Mode::Unattended);
        fs::set_permissions(&previous, fs::Permissions::from_mode(0o700)).unwrap();

        let installed = installed.unwrap();
        assert!(matches!(installed.previous, Previous::Stranded { .. }));
        assert_eq!(
            macos::bundle_version(&older).unwrap(),
            Version::new(1, 5, 0)
        );
    }

    #[test]
    fn a_rejected_release_is_not_downloaded_again() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        // The archive's fault: it holds another version than its manifest.
        let (release, bytes) = world.release("1.7.0", "1.6.5");
        assert!(may_download_unattended(&installer, &release, 1_000).unwrap());
        assert!(installer.stage(&release, &bytes).is_err());

        // Six hours later, or after a paired machine's hint: still skipped.
        let later = 1_000 + 7 * 3600;
        assert!(installer.rejected(&release).unwrap());
        assert!(!may_download_unattended(&installer, &release, later).unwrap());
        // The next release is downloaded.
        let (next, _) = world.release("1.7.1", "1.7.1");
        assert!(may_download_unattended(&installer, &next, later).unwrap());
    }

    #[test]
    fn unattended_downloads_wait_out_their_cooldown_until_an_install() {
        let world = World::new();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert!(may_download_unattended(&installer, &release, 1_000).unwrap());
        assert!(!may_download_unattended(&installer, &release, 1_060).unwrap());
        assert!(may_download_unattended(&installer, &release, 1_000 + 6 * 3600).unwrap());

        let staged = installer.stage(&release, &bytes).unwrap();
        installer.replace(staged, Mode::Unattended).unwrap();
        let state = StateStore::new(&world.paths).load().unwrap();
        assert_eq!(state.last_download_at, None);
    }

    #[test]
    fn a_corrupt_state_file_stops_the_install_before_any_download() {
        let world = World::new();
        fs::create_dir_all(world.paths.dir()).unwrap();
        fs::write(world.paths.state_file(), b"{\"highestInstalled\": 1").unwrap();
        let installer = world.installer(&Signed(true), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert!(installer.wants(&release).is_err());
        assert!(may_download_unattended(&installer, &release, 1_000).is_err());
        assert!(installer.stage(&release, &bytes).is_err());
        assert_eq!(
            fs::read(world.paths.state_file()).unwrap(),
            b"{\"highestInstalled\": 1"
        );
    }
}
