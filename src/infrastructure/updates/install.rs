//! Localify's one installer (docs/desktop.md, "Automatic updates"): the
//! daemon runs it unattended, and `localify update install` when a person
//! asks. Every rule of the contract is enforced here, in this order:
//! signature, extraction, version, code signature, then, under the update
//! lock, the app checks, the destination check, and the swap.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

use super::UpdatePaths;
use super::extract::{self, LIMITS};
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

impl Installer<'_> {
    /// Whether `release` may replace the installed bundle at all: strictly
    /// newer than it and than every version installed here.
    pub fn wants(&self, release: &Release) -> Result<Option<Version>> {
        let offered = parse_version(&release.version)
            .ok_or_else(|| anyhow!("The update manifest has an invalid version"))?;
        let highest = StateStore::new(self.paths).load().highest();
        let newer = rules::offer_is_newer(&offered, &self.target.version, highest.as_ref());
        Ok(newer.then_some(offered))
    }

    /// Checks the archive and expands it into a new private directory: the
    /// signature before anything is extracted, then the bundle's version
    /// and its code signature. An archive that fails any of these is
    /// recorded by its SHA-256 and skipped until another release.
    pub fn stage(&self, release: &Release, archive: &[u8]) -> Result<Staged> {
        let digest = release::sha256_hex(archive);
        let store = StateStore::new(self.paths);
        if store.load().has_failed(&digest) {
            bail!(
                "Localify {} failed to install here before. Waiting for the next release.",
                release.version
            );
        }
        let staged = self.verify_and_expand(release, archive);
        if let Err(error) = &staged {
            self.log(&format!("rejected sha256={digest}: {error:#}"));
            store.update(self.paths, |state| state.record_failed(&digest))?;
        }
        staged
    }

    fn verify_and_expand(&self, release: &Release, archive: &[u8]) -> Result<Staged> {
        let manifest_archive = release
            .archive
            .as_ref()
            .ok_or_else(|| anyhow!("The update manifest has no archive for this Mac"))?;
        release::verify_signature(archive, &manifest_archive.signature, self.public_key)?;
        let dir = StagingDir::create(self.paths, self.target.device())?;
        let bundle = extract::extract_bundle(archive, dir.path(), LIMITS)
            .context("The update archive was refused")?;
        let version = macos::bundle_version(&bundle)?;
        if version.to_string() != release.version {
            bail!(
                "The update archive holds version {version}, not the {} the manifest names",
                release.version
            );
        }
        self.signature.verify(&bundle)?;
        Ok(Staged {
            dir,
            bundle,
            version,
        })
    }

    /// Swaps the staged bundle in, under the update lock, once the app is
    /// closed and the destination is still the one checked. The old bundle
    /// is kept in `previous/`.
    pub fn replace(&self, staged: Staged, mode: Mode) -> Result<Version> {
        let _held = self.take_locks(mode)?;
        let bundle_use = rules::bundle_use(&self.target.bundle, &(self.processes)()?);
        rules::app_is_closed(true, bundle_use)?;
        if mode == Mode::Unattended && !(self.automatic)() {
            bail!("Automatic updates were turned off, so nothing was replaced.");
        }
        self.target.still_unchanged()?;
        macos::swap(&staged.bundle, &self.target.bundle)?;
        self.log(&format!(
            "installed version={} previous={} at={}",
            staged.version,
            self.target.version,
            self.target.bundle.display()
        ));
        self.keep_previous(&staged.bundle);
        StateStore::new(self.paths).update(self.paths, |state| {
            state.saw_installed(&staged.version);
            state.install_error = None;
        })?;
        drop(staged.dir);
        Ok(staged.version)
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

    /// Moves the bundle swapped out to `previous/<version>.app`, replacing
    /// the one kept before. A failure only costs the copy.
    fn keep_previous(&self, old_bundle: &Path) {
        let previous = self.paths.previous_dir();
        let kept = previous.join(format!("{}.app", self.target.version));
        let moved = remove_dir_contents(&previous)
            .and_then(|()| fs::create_dir_all(&previous))
            .and_then(|()| fs::rename(old_bundle, &kept));
        match moved {
            Ok(()) => self.log(&format!("kept previous={}", kept.display())),
            Err(error) => self.log(&format!("could not keep the previous bundle: {error}")),
        }
    }

    pub fn log(&self, message: &str) {
        super::log(self.paths, message);
    }
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

fn remove_dir_contents(dir: &Path) -> std::io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if fs::symlink_metadata(&path)?.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// A new 0700 directory under the updates folder, on the destination's
/// volume, removed with what it holds when dropped.
struct StagingDir(PathBuf);

impl StagingDir {
    fn create(paths: &UpdatePaths, device: u64) -> Result<Self> {
        let root = paths.staging_root();
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .with_context(|| format!("Could not create {}", root.display()))?;
        let name = format!("{}-{}", std::process::id(), super::random_u64());
        let dir = root.join(name);
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
        let staging = Self(dir);
        if fs::metadata(&staging.0)?.dev() != device {
            bail!(
                "{} is on another volume than the installed app, so the update can't be swapped in.",
                staging.0.display()
            );
        }
        Ok(staging)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::infrastructure::updates::extract::tests::{Item, archive};
    use crate::infrastructure::updates::macos::tests::fake_bundle;
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

        let staged = installer.stage(&release, &bytes).unwrap();
        let installed = installer.replace(staged, Mode::Unattended).unwrap();

        assert_eq!(installed, Version::new(1, 7, 0));
        assert_eq!(
            fs::read(world.bundle().join(CLI_IN_BUNDLE)).unwrap(),
            b"new"
        );
        let kept = world.paths.previous_dir().join("1.6.0.app");
        assert_eq!(macos::bundle_version(&kept).unwrap(), Version::new(1, 6, 0));
        assert_eq!(fs::read_dir(world.paths.staging_root()).unwrap().count(), 0);
        let state = StateStore::new(&world.paths).load();
        assert_eq!(state.highest(), Some(Version::new(1, 7, 0)));
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
        assert!(StateStore::new(&world.paths).load().has_failed(&digest));

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
        assert!(StateStore::new(&world.paths).load().has_failed(&digest));
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
            installer.replace(staged, wait).unwrap(),
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
}
