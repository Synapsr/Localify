//! The app's lifetime lock (docs/desktop.md, "Automatic updates"). The app
//! takes `app.lock` as its first step and holds it until it exits, so the
//! installer never replaces the bundle under it.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::UpdatePaths;

/// How often a waiting app tries again.
const POLL: Duration = Duration::from_millis(200);
/// A swap takes a second or two: the app says it is updating only past this.
pub const MESSAGE_AFTER: Duration = Duration::from_secs(1);
/// An installer lets go of `update.lock` just before `app.lock`, and the
/// daemon's check takes `app.lock` alone for an instant: `app.lock` held
/// with `update.lock` free is tried again this many times before it counts
/// as another app.
const RELEASE_TRIES: u32 = 5;
const RELEASE_POLL: Duration = Duration::from_millis(20);

/// What the app's start found.
#[derive(Debug)]
pub enum AppLock {
    /// The app holds the lock for as long as it keeps `file` open.
    Held(File),
    /// Another copy of the app holds it. This copy hands its launch over to
    /// that one, so it never waits for it.
    HeldByAnotherApp,
}

/// Takes `app.lock`. An installer holds it only while it also holds
/// `update.lock`, so a lock held without `update.lock` is another copy of
/// the app, once it stays held a little. While an install holds it, this
/// waits, and calls `on_wait` once the wait passes [`MESSAGE_AFTER`].
///
/// Looking at `update.lock` takes it for an instant when it is free. An
/// unattended install that tries it at that instant gives up until its next
/// check, which it would do anyway, since the app holds `app.lock`.
pub fn take_app_lock(paths: &UpdatePaths, on_wait: impl FnOnce()) -> Result<AppLock> {
    take(paths, on_wait, || {})
}

/// [`take_app_lock`], with `between_probes` called after `app.lock` was
/// found held and before `update.lock` is looked at: tests end an install
/// there.
fn take(
    paths: &UpdatePaths,
    on_wait: impl FnOnce(),
    mut between_probes: impl FnMut(),
) -> Result<AppLock> {
    let app_lock = paths.app_lock();
    let file = open_lock(&app_lock)?;
    let started = Instant::now();
    let mut on_wait = Some(on_wait);
    loop {
        if try_lock(&file, &app_lock)? {
            return Ok(AppLock::Held(file));
        }
        between_probes();
        if !is_held(&paths.update_lock())? {
            // No install now: one may have just ended, letting go of both.
            for _ in 0..RELEASE_TRIES {
                if try_lock(&file, &app_lock)? {
                    return Ok(AppLock::Held(file));
                }
                std::thread::sleep(RELEASE_POLL);
            }
            if !is_held(&paths.update_lock())? {
                return Ok(AppLock::HeldByAnotherApp);
            }
        }
        if started.elapsed() >= MESSAGE_AFTER
            && let Some(on_wait) = on_wait.take()
        {
            on_wait();
        }
        std::thread::sleep(POLL);
    }
}

fn try_lock(file: &File, path: &Path) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("Could not lock {}", path.display()))
        }
    }
}

fn is_held(path: &Path) -> Result<bool> {
    Ok(!try_lock(&open_lock(path)?, path)?)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use super::*;

    fn paths() -> (tempfile::TempDir, UpdatePaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::new(dir.path().join("Localify"));
        (dir, paths)
    }

    fn held(path: &Path) -> File {
        let file = open_lock(path).unwrap();
        file.lock().unwrap();
        file
    }

    #[test]
    fn a_free_lock_is_taken_and_kept_until_the_file_closes() {
        let (_dir, paths) = paths();
        let AppLock::Held(file) = take_app_lock(&paths, || {}).unwrap() else {
            panic!("the lock was free");
        };
        assert!(is_held(&paths.app_lock()).unwrap());
        drop(file);
        assert!(!is_held(&paths.app_lock()).unwrap());
    }

    #[test]
    fn another_app_holding_it_is_not_waited_for() {
        let (_dir, paths) = paths();
        let _other_app = held(&paths.app_lock());
        let called = AtomicBool::new(false);
        let lock = take_app_lock(&paths, || called.store(true, Ordering::SeqCst)).unwrap();
        assert!(matches!(lock, AppLock::HeldByAnotherApp));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn an_install_holding_both_locks_is_waited_for_with_a_message() {
        let (_dir, paths) = paths();
        let update = held(&paths.update_lock());
        let app = held(&paths.app_lock());
        let (said, heard) = mpsc::channel();
        let installer = std::thread::spawn(move || {
            // The message comes once the wait is long enough, then the
            // install ends.
            heard.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(app);
            drop(update);
        });
        let lock = take_app_lock(&paths, move || said.send(()).unwrap()).unwrap();
        installer.join().unwrap();
        assert!(matches!(lock, AppLock::Held(_)), "the install ended");
        assert!(is_held(&paths.app_lock()).unwrap());
    }

    #[test]
    fn a_short_install_ends_before_any_message() {
        let (_dir, paths) = paths();
        let update = held(&paths.update_lock());
        let app = held(&paths.app_lock());
        let installer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(app);
            drop(update);
        });
        let called = AtomicBool::new(false);
        let lock = take_app_lock(&paths, || called.store(true, Ordering::SeqCst)).unwrap();
        installer.join().unwrap();
        assert!(matches!(lock, AppLock::Held(_)));
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn an_install_that_ends_between_the_two_probes_is_not_another_app() {
        let (_dir, paths) = paths();
        let mut installer = Some((held(&paths.update_lock()), held(&paths.app_lock())));
        // The installer lets go of both locks right after `app.lock` was
        // found held, and before `update.lock` is looked at.
        let lock = take(&paths, || {}, || drop(installer.take())).unwrap();
        assert!(matches!(lock, AppLock::Held(_)));
        assert!(is_held(&paths.app_lock()).unwrap());
    }
}
