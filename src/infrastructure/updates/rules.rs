//! The decisions of automatic updates that need no I/O
//! (docs/desktop.md, "Automatic updates"), kept apart so tests pin them.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;

/// The first check after the daemon starts.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(2 * 60);
/// Between scheduled checks, before the jitter.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Each scheduled check moves by up to this much either way, so machines
/// started together don't all ask at once.
pub const CHECK_JITTER: Duration = Duration::from_secs(30 * 60);
/// The machine-wide wait after a check, doubled after each failure.
pub const BASE_COOLDOWN: Duration = Duration::from_secs(10 * 60);
pub const MAX_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
/// The wait after an unattended download that didn't end in an install, so
/// checks and hints can't download the same release again and again.
pub const DOWNLOAD_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);

/// Reads a release version, with or without a leading `v`.
pub fn parse_version(value: &str) -> Option<Version> {
    let value = value.trim();
    Version::parse(value.strip_prefix('v').unwrap_or(value)).ok()
}

/// How long after a check the next may reach the network: 10 minutes after
/// a success, doubled after each failure in a row, up to 6 hours.
pub fn cooldown(failures: u32) -> Duration {
    let factor = 1u64 << failures.min(16);
    let secs = BASE_COOLDOWN.as_secs().saturating_mul(factor);
    Duration::from_secs(secs.min(MAX_COOLDOWN.as_secs()))
}

/// Whether a check may reach the network now, from the persisted time of the
/// last one (Unix seconds). A clock moved back counts as due.
pub fn check_is_due(now: u64, last_check_at: Option<u64>, failures: u32) -> bool {
    waited(now, last_check_at, cooldown(failures))
}

/// Whether the daemon may download a release now, from the persisted time
/// of its last unattended download.
pub fn download_is_due(now: u64, last_download_at: Option<u64>) -> bool {
    waited(now, last_download_at, DOWNLOAD_COOLDOWN)
}

fn waited(now: u64, last: Option<u64>, wait: Duration) -> bool {
    match last {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now - last >= wait.as_secs(),
    }
}

/// Whether `a` is a newer release than `b`. Build metadata (`+...`) is not
/// part of a version's precedence, so `1.7.0+2` is not newer than `1.7.0`.
pub fn is_newer(a: &Version, b: &Version) -> bool {
    a.cmp_precedence(b) == Ordering::Greater
}

/// The wait before the next scheduled check: 6 hours plus or minus 30
/// minutes, picked from `seed`.
pub fn next_check_delay(seed: u64) -> Duration {
    let jitter = CHECK_JITTER.as_secs();
    let offset = seed % (2 * jitter + 1);
    Duration::from_secs(CHECK_INTERVAL.as_secs() - jitter + offset)
}

/// Whether an offered version may be installed: strictly newer than the
/// installed bundle and than every version this machine ever installed, so
/// an older valid release is never put back.
pub fn offer_is_newer(offered: &Version, installed: &Version, highest: Option<&Version>) -> bool {
    is_newer(offered, installed) && highest.is_none_or(|highest| is_newer(offered, highest))
}

/// The newest version paired machines report, when it is newer than ours.
/// It comes from the relay's unauthenticated presence: it only makes this
/// machine check early, never chooses what to install.
pub fn peer_hint<'a>(
    peer_versions: impl IntoIterator<Item = &'a str>,
    installed: &Version,
) -> Option<Version> {
    peer_versions
        .into_iter()
        .filter_map(parse_version)
        .filter(|version| is_newer(version, installed))
        .max_by(Version::cmp_precedence)
}

/// What `proc_pidpath` said about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessPath {
    Path(PathBuf),
    /// It exited between the listing and the read.
    Gone,
    /// Its executable was deleted, so it can't be a file of the bundle on
    /// disk now. Apps that updated themselves while running look like this.
    Deleted,
    /// Any other failure: nothing can be said about it.
    Unreadable,
}

/// Whether some process is running from the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleUse {
    Free,
    InUse { pid: i32, path: PathBuf },
    Unknown { pid: i32 },
}

/// Looks for a process whose executable lies inside `bundle`, other than
/// the bundle's own `localify` binary: the daemon and the CLI run from it
/// and are built to outlive a replacement (the daemon restarts on the new
/// build). A process whose path can't be read makes the answer unknown.
pub fn bundle_use(bundle: &Path, processes: &[(i32, ProcessPath)]) -> BundleUse {
    let cli = bundle.join("Contents/MacOS/localify");
    let mut unknown = None;
    for (pid, path) in processes {
        match path {
            ProcessPath::Path(path) if path.starts_with(bundle) && path != &cli => {
                return BundleUse::InUse {
                    pid: *pid,
                    path: path.clone(),
                };
            }
            ProcessPath::Unreadable => unknown = unknown.or(Some(*pid)),
            _ => {}
        }
    }
    match unknown {
        Some(pid) => BundleUse::Unknown { pid },
        None => BundleUse::Free,
    }
}

/// Why the installer won't replace the bundle now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppRunning {
    #[error("Localify is open. Quit it, or use Restart to update in the app.")]
    LockHeld,
    #[error("Localify is running from the bundle (process {pid}, {}).", path.display())]
    Process { pid: i32, path: PathBuf },
    #[error("Could not tell whether process {pid} runs from the bundle, so nothing was replaced.")]
    Unknown { pid: i32 },
}

/// The installer runs only when it holds `app.lock` and no process runs
/// from the bundle; when unsure, it doesn't.
pub fn app_is_closed(app_lock_taken: bool, bundle_use: BundleUse) -> Result<(), AppRunning> {
    if !app_lock_taken {
        return Err(AppRunning::LockHeld);
    }
    match bundle_use {
        BundleUse::Free => Ok(()),
        BundleUse::InUse { pid, path } => Err(AppRunning::Process { pid, path }),
        BundleUse::Unknown { pid } => Err(AppRunning::Unknown { pid }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(value: &str) -> Version {
        parse_version(value).unwrap()
    }

    #[test]
    fn versions_parse_with_or_without_a_v() {
        assert_eq!(parse_version("v1.7.0"), Some(v("1.7.0")));
        assert_eq!(parse_version(" 1.7.0 "), Some(v("1.7.0")));
        assert_eq!(parse_version("1.7"), None);
    }

    #[test]
    fn only_a_strictly_newer_version_is_offered() {
        let installed = v("1.6.0");
        assert!(offer_is_newer(&v("1.6.1"), &installed, None));
        assert!(!offer_is_newer(&v("1.6.0"), &installed, None));
        assert!(!offer_is_newer(&v("1.5.9"), &installed, None));
        assert!(offer_is_newer(&v("1.7.0-beta.1"), &installed, None));
    }

    #[test]
    fn build_metadata_never_makes_a_version_newer() {
        let installed = v("1.7.0+1");
        assert!(!offer_is_newer(&v("1.7.0+2"), &installed, None));
        assert!(!offer_is_newer(&v("1.7.0+0"), &installed, None));
        assert!(!offer_is_newer(
            &v("1.7.0+zz"),
            &v("1.6.0"),
            Some(&v("1.7.0"))
        ));
        assert!(offer_is_newer(
            &v("1.7.1+0"),
            &installed,
            Some(&v("1.7.0+9"))
        ));
        assert_eq!(peer_hint(["1.7.0+9", "1.6.0"], &installed), None);
    }

    #[test]
    fn an_unattended_download_waits_six_hours_after_the_last() {
        assert!(download_is_due(1_000, None));
        assert!(!download_is_due(1_000 + 6 * 3600 - 1, Some(1_000)));
        assert!(download_is_due(1_000 + 6 * 3600, Some(1_000)));
        assert!(download_is_due(500, Some(1_000)));
    }

    #[test]
    fn a_version_below_the_highest_ever_installed_is_a_downgrade() {
        // Someone put 1.6 back by hand after 1.8: 1.7 is still refused.
        let installed = v("1.6.0");
        let highest = v("1.8.0");
        assert!(!offer_is_newer(&v("1.7.0"), &installed, Some(&highest)));
        assert!(!offer_is_newer(&v("1.8.0"), &installed, Some(&highest)));
        assert!(offer_is_newer(&v("1.8.1"), &installed, Some(&highest)));
    }

    #[test]
    fn the_cooldown_doubles_after_each_failure_up_to_six_hours() {
        assert_eq!(cooldown(0), Duration::from_secs(600));
        assert_eq!(cooldown(1), Duration::from_secs(1200));
        assert_eq!(cooldown(3), Duration::from_secs(4800));
        assert_eq!(cooldown(6), MAX_COOLDOWN);
        assert_eq!(cooldown(u32::MAX), MAX_COOLDOWN);
    }

    #[test]
    fn a_check_is_due_once_the_cooldown_passed() {
        assert!(check_is_due(1_000, None, 0));
        assert!(!check_is_due(1_000 + 599, Some(1_000), 0));
        assert!(check_is_due(1_000 + 600, Some(1_000), 0));
        assert!(!check_is_due(1_000 + 1_199, Some(1_000), 1));
        assert!(check_is_due(1_000 + 1_200, Some(1_000), 1));
        // A clock moved back doesn't block checks for good.
        assert!(check_is_due(500, Some(1_000), 0));
    }

    #[test]
    fn scheduled_checks_stay_within_half_an_hour_of_six_hours() {
        let low = CHECK_INTERVAL - CHECK_JITTER;
        let high = CHECK_INTERVAL + CHECK_JITTER;
        assert_eq!(next_check_delay(0), low);
        assert_eq!(next_check_delay(2 * CHECK_JITTER.as_secs()), high);
        for seed in [1, 77, 12_345, u64::MAX] {
            let delay = next_check_delay(seed);
            assert!(delay >= low && delay <= high);
        }
    }

    #[test]
    fn a_peer_on_a_newer_version_is_a_hint() {
        let installed = v("1.6.0");
        assert_eq!(
            peer_hint(["1.5.0", "1.7.0", "garbage", "1.6.2"], &installed),
            Some(v("1.7.0"))
        );
        assert_eq!(peer_hint(["1.6.0", "1.5.0"], &installed), None);
    }

    fn bundle() -> PathBuf {
        PathBuf::from("/Applications/Localify.app")
    }

    #[test]
    fn the_bundles_own_cli_and_other_apps_leave_it_free() {
        let processes = [
            (
                10,
                ProcessPath::Path(bundle().join("Contents/MacOS/localify")),
            ),
            (
                11,
                ProcessPath::Path("/Applications/Localify.app.old/x".into()),
            ),
            (12, ProcessPath::Path("/usr/bin/true".into())),
            (13, ProcessPath::Gone),
            (14, ProcessPath::Deleted),
        ];
        assert_eq!(bundle_use(&bundle(), &processes), BundleUse::Free);
    }

    #[test]
    fn the_app_running_from_the_bundle_is_found() {
        let app = bundle().join("Contents/MacOS/localify-desktop");
        let processes = [
            (20, ProcessPath::Unreadable),
            (21, ProcessPath::Path(app.clone())),
        ];
        assert_eq!(
            bundle_use(&bundle(), &processes),
            BundleUse::InUse { pid: 21, path: app }
        );
    }

    #[test]
    fn an_unreadable_process_makes_the_answer_unknown() {
        let processes = [(30, ProcessPath::Unreadable)];
        assert_eq!(
            bundle_use(&bundle(), &processes),
            BundleUse::Unknown { pid: 30 }
        );
    }

    #[test]
    fn the_installer_needs_the_lock_and_a_free_bundle() {
        assert_eq!(app_is_closed(true, BundleUse::Free), Ok(()));
        assert_eq!(
            app_is_closed(false, BundleUse::Free),
            Err(AppRunning::LockHeld)
        );
        assert!(matches!(
            app_is_closed(true, BundleUse::Unknown { pid: 3 }),
            Err(AppRunning::Unknown { pid: 3 })
        ));
        assert!(matches!(
            app_is_closed(
                true,
                BundleUse::InUse {
                    pid: 4,
                    path: "/x".into()
                }
            ),
            Err(AppRunning::Process { pid: 4, .. })
        ));
    }
}
