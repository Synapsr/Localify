//! Localify's one installer (docs/desktop.md, "Automatic updates"): the
//! daemon runs it unattended, and `localify update install` when a person
//! asks. Every rule of the contract is enforced here, in this order:
//! signature, extraction, version, code signature, then, under the update
//! lock, the app checks, the destination check, the staged bundle checked
//! again, and the swap.

use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
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
    /// and its code signature. An archive that fails any of these is
    /// recorded, by its SHA-256 and by the release's identity, and skipped
    /// until another release.
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
        let bundle = match extract::extract_bundle(archive, dir.path(), LIMITS) {
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
        let version = macos::bundle_version(&bundle).map_err(Refusal::Archive)?;
        if version.to_string() != release.version {
            return Err(Refusal::Archive(anyhow!(
                "The update archive holds version {version}, not the {} the manifest names",
                release.version
            )));
        }
        self.signature.verify(&bundle).map_err(Refusal::Archive)?;
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
        macos::swap(&staged.bundle, &self.target.bundle)?;
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
    /// the same version, and its code signature.
    fn still_verified(&self, staged: &Staged) -> Result<()> {
        let changed = "The staged update changed after it was verified, so nothing was replaced";
        let version = macos::bundle_version(&staged.bundle).context(changed)?;
        if version != staged.version {
            bail!("{changed}: it is now version {version}.");
        }
        self.signature.verify(&staged.bundle).context(changed)
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
/// volume, removed with what it holds when dropped unless kept.
struct StagingDir {
    path: PathBuf,
    keep: bool,
}

impl StagingDir {
    /// Only this account may write in it: the updates folder, `staging/`
    /// and the new directory are owned by it, writable by nobody else by
    /// their mode, and have no ACL, inherited or not. The new directory is
    /// checked empty once that holds.
    fn create(paths: &UpdatePaths, device: u64) -> Result<Self> {
        fs::create_dir_all(paths.dir())
            .with_context(|| format!("Could not create {}", paths.dir().display()))?;
        make_private(paths.dir(), 0o755)?;
        let root = paths.staging_root();
        match DirBuilder::new().mode(0o700).create(&root) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                return Err(error).with_context(|| format!("Could not create {}", root.display()));
            }
            _ => {}
        }
        make_private(&root, 0o700)?;
        let name = format!("{}-{}", std::process::id(), super::random_u64());
        let dir = root.join(name);
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        let staging = Self {
            path: dir,
            keep: false,
        };
        let opened = make_private(&staging.path, 0o700)?;
        if fs::read_dir(&staging.path)?.next().is_some() {
            bail!(
                "Something else wrote in {}, so the update wasn't staged there.",
                staging.path.display()
            );
        }
        if opened.metadata()?.dev() != device {
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

/// Opens the directory at `path`, never through a symlink, checks that this
/// account owns it, then lets only the owner write in it: its mode keeps at
/// most `keep` (and always the owner's access), and its ACL is cleared.
fn make_private(path: &Path, keep: u32) -> Result<File> {
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("Could not open {}", path.display()))?;
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
        macos::clear_acl(&dir)
    };
    private().with_context(|| format!("Could not make {} private", path.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::infrastructure::updates::extract::tests::{Item, archive};
    use crate::infrastructure::updates::macos::tests::{fake_bundle, has_acl};
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
    fn a_bundle_not_signed_by_localify_is_refused_and_remembered() {
        let world = World::new();
        let installer = world.installer(&Signed(false), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
        assert!(installer.stage(&release, &bytes).is_err());
        let digest = release::sha256_hex(&bytes);
        assert!(
            StateStore::new(&world.paths)
                .load()
                .unwrap()
                .has_failed(&digest)
        );
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
        // Anyone may add files below, and new folders inherit that.
        let status = std::process::Command::new("/bin/chmod")
            .args([
                "+a",
                "everyone allow add_file,add_subdirectory,delete_child,file_inherit,directory_inherit",
            ])
            .arg(&world.root)
            .status()
            .unwrap();
        assert!(status.success());
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
        let installer = world.installer(&Signed(false), &none, &|| true);
        let (release, bytes) = world.release("1.7.0", "1.7.0");
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
