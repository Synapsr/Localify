//! Who a root process acts as, and how it stops being root.
//!
//! The daemon needs root for one thing: binding low ports. Everything it serves
//! afterwards runs as the user who installed it, so a route can reach only what
//! that user could already reach. A root process that cannot name that user does
//! not run the daemon at all.

use std::ffi::{CStr, CString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;

/// The account a root process hands its work to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnprivilegedUser {
    pub uid: u32,
    pub gid: u32,
    pub name: CString,
    pub home: PathBuf,
}

impl UnprivilegedUser {
    /// Reads the account from the system user database, never the environment.
    /// Refuses root and root's primary group: dropping to them drops nothing.
    pub fn lookup(uid: u32) -> Result<Self> {
        if uid == 0 {
            bail!(
                "The {} daemon must run as a non-root user, but UID 0 was given.",
                BRANDING.app_name
            );
        }

        let user = lookup_account(uid)?;
        if user.gid == 0 {
            bail!(
                "UID {uid} has the root group as its primary group, so the {} daemon cannot drop its privileges to it.",
                BRANDING.app_name
            );
        }
        Ok(user)
    }
}

/// The IDs a process holds, to check a drop really happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIds {
    pub uid: u32,
    pub euid: u32,
    pub gid: u32,
    pub egid: u32,
}

/// The system calls behind a privilege drop, separate so tests can run the
/// sequence and its failure paths without being root.
pub trait Credentials {
    fn init_groups(&mut self, name: &CStr, gid: u32) -> io::Result<()>;
    fn set_gid(&mut self, gid: u32) -> io::Result<()>;
    fn set_uid(&mut self, uid: u32) -> io::Result<()>;
    fn ids(&self) -> ProcessIds;
}

/// The real system calls.
pub struct ProcessCredentials;

impl Credentials for ProcessCredentials {
    fn init_groups(&mut self, name: &CStr, gid: u32) -> io::Result<()> {
        // SAFETY: `name` is a valid C string for the duration of the call. The
        // gid parameter is `int` on macOS and `gid_t` on Linux.
        check_result(unsafe { libc::initgroups(name.as_ptr(), gid as _) })
    }

    fn set_gid(&mut self, gid: u32) -> io::Result<()> {
        // SAFETY: setgid takes a plain integer and touches no memory of ours.
        check_result(unsafe { libc::setgid(gid) })
    }

    fn set_uid(&mut self, uid: u32) -> io::Result<()> {
        // SAFETY: setuid takes a plain integer and touches no memory of ours.
        check_result(unsafe { libc::setuid(uid) })
    }

    fn ids(&self) -> ProcessIds {
        // SAFETY: these calls take no arguments and cannot fail.
        unsafe {
            ProcessIds {
                uid: libc::getuid(),
                euid: libc::geteuid(),
                gid: libc::getgid(),
                egid: libc::getegid(),
            }
        }
    }
}

fn check_result(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Permanently becomes `user`, then proves root cannot be regained.
///
/// Groups go first and the UID last, because only root can change groups. Call
/// this while the process has a single thread. A caller must treat an error as
/// fatal: the process may be partly dropped, and it must not go on to serve.
pub fn drop_privileges(credentials: &mut impl Credentials, user: &UnprivilegedUser) -> Result<()> {
    credentials
        .init_groups(&user.name, user.gid)
        .context("Failed to set the user's supplementary groups")?;
    credentials
        .set_gid(user.gid)
        .context("Failed to set the user's group")?;
    credentials
        .set_uid(user.uid)
        .context("Failed to set the user ID")?;

    if credentials.set_uid(0).is_ok() || credentials.set_gid(0).is_ok() {
        bail!(
            "The {} daemon could regain root after dropping to UID {}.",
            BRANDING.app_name,
            user.uid
        );
    }

    let expected = ProcessIds {
        uid: user.uid,
        euid: user.uid,
        gid: user.gid,
        egid: user.gid,
    };
    let actual = credentials.ids();
    if actual != expected {
        bail!(
            "The {} daemon holds IDs {actual:?} after its privilege drop, expected {expected:?}.",
            BRANDING.app_name
        );
    }
    Ok(())
}

/// Points the environment at the account the process just became, so anything
/// resolving `~` finds the user's files and not root's.
///
/// # Safety
///
/// The process must have a single thread: changing the environment is not
/// thread-safe.
pub unsafe fn adopt_user_environment(user: &UnprivilegedUser) {
    let name = user.name.to_string_lossy().into_owned();
    // SAFETY: the caller guarantees no other thread reads the environment.
    unsafe {
        std::env::set_var("HOME", &user.home);
        std::env::set_var("USER", &name);
        std::env::set_var("LOGNAME", &name);
    }
}

pub fn running_as_root() -> bool {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[allow(dead_code)] // Used by the system helper through the library crate.
pub fn current_real_uid() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// Decides who the daemon runs as: `None` when it is not root and has nothing to
/// drop. A root process with nobody to become gets an error rather than a root
/// daemon, because the daemon parses network input and serves files.
pub fn daemon_uid(
    euid: u32,
    run_as_uid: Option<u32>,
    sudo_uid: Option<&str>,
) -> Result<Option<u32>> {
    if euid != 0 {
        return Ok(None);
    }

    if let Some(uid) = run_as_uid {
        return Ok(Some(uid));
    }

    match sudo_uid.and_then(|uid| uid.trim().parse::<u32>().ok()) {
        Some(uid) if uid != 0 => Ok(Some(uid)),
        _ => bail!(
            "Refusing to run the {app} daemon as root: there is no user to run it as.\n\
             Start it with sudo from your own account, or pass --run-as-uid <uid>.",
            app = BRANDING.app_name
        ),
    }
}

/// The user this root process works for. The helper sets it from `--uid`.
#[allow(dead_code)] // Used through the library crate.
static ACTING_FOR: OnceLock<u32> = OnceLock::new();

/// Records whom a long-running root process acts for.
#[allow(dead_code)] // Used through the library crate.
pub fn set_acting_user(uid: u32) {
    let _ = ACTING_FOR.set(uid);
}

/// Makes a child of a root process run as the user it acts for, for a command
/// that handles that user's files or signals that user's processes. Does nothing
/// when this process is not root. A root process with no known user leaves the
/// command unchanged, which only an administrator's own shell can reach.
#[allow(dead_code)] // Used through the library crate.
pub fn run_as_acting_user(command: &mut Command) {
    if !running_as_root() {
        return;
    }

    let uid = ACTING_FOR.get().copied().or_else(|| {
        std::env::var("SUDO_UID")
            .ok()
            .and_then(|uid| uid.trim().parse::<u32>().ok())
    });
    let Some(user) = uid.and_then(|uid| UnprivilegedUser::lookup(uid).ok()) else {
        return;
    };

    command.uid(user.uid).gid(user.gid).env("HOME", &user.home);
}

fn lookup_account(uid: u32) -> Result<UnprivilegedUser> {
    // SAFETY: zeroed is a valid value for this plain C struct, and getpwuid_r
    // fills it.
    let mut record: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut found: *mut libc::passwd = std::ptr::null_mut();

    // SAFETY: every pointer is valid for the call and `buffer.len()` is the
    // size of `buffer`. The strings it writes stay in `buffer` until we copy
    // them below.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut record,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status))
            .with_context(|| format!("Failed to look up UID {uid} in the user database"));
    }
    if found.is_null() {
        bail!("UID {uid} is not in the user database.");
    }

    // SAFETY: getpwuid_r succeeded, so both fields point at C strings in `buffer`.
    let (name, home) = unsafe {
        (
            CStr::from_ptr(record.pw_name).to_owned(),
            CStr::from_ptr(record.pw_dir).to_owned(),
        )
    };
    Ok(UnprivilegedUser {
        uid,
        gid: record.pw_gid,
        name,
        home: PathBuf::from(std::ffi::OsStr::from_bytes(home.as_bytes())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the calls, and fails or succeeds as a test needs.
    struct FakeCredentials {
        calls: Vec<String>,
        regain_succeeds: bool,
        fail_set_uid: bool,
        ids: ProcessIds,
    }

    impl FakeCredentials {
        fn new(ids: ProcessIds) -> Self {
            Self {
                calls: Vec::new(),
                regain_succeeds: false,
                fail_set_uid: false,
                ids,
            }
        }
    }

    impl Credentials for FakeCredentials {
        fn init_groups(&mut self, _name: &CStr, gid: u32) -> io::Result<()> {
            self.calls.push(format!("init_groups {gid}"));
            Ok(())
        }

        fn set_gid(&mut self, gid: u32) -> io::Result<()> {
            self.calls.push(format!("set_gid {gid}"));
            if gid == 0 && !self.regain_succeeds {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
            Ok(())
        }

        fn set_uid(&mut self, uid: u32) -> io::Result<()> {
            self.calls.push(format!("set_uid {uid}"));
            if self.fail_set_uid || (uid == 0 && !self.regain_succeeds) {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
            Ok(())
        }

        fn ids(&self) -> ProcessIds {
            self.ids
        }
    }

    fn user() -> UnprivilegedUser {
        UnprivilegedUser {
            uid: 501,
            gid: 20,
            name: CString::new("dev").unwrap(),
            home: PathBuf::from("/Users/dev"),
        }
    }

    fn dropped_ids() -> ProcessIds {
        ProcessIds {
            uid: 501,
            euid: 501,
            gid: 20,
            egid: 20,
        }
    }

    #[test]
    fn a_drop_sets_groups_then_gid_then_uid_and_tries_to_regain_root() {
        let mut credentials = FakeCredentials::new(dropped_ids());

        drop_privileges(&mut credentials, &user()).unwrap();

        assert_eq!(
            credentials.calls,
            [
                "init_groups 20",
                "set_gid 20",
                "set_uid 501",
                "set_uid 0",
                "set_gid 0"
            ]
        );
    }

    #[test]
    fn a_drop_fails_when_root_can_be_regained() {
        let mut credentials = FakeCredentials::new(dropped_ids());
        credentials.regain_succeeds = true;

        let error = drop_privileges(&mut credentials, &user()).unwrap_err();

        assert!(error.to_string().contains("could regain root"));
    }

    #[test]
    fn a_drop_fails_when_the_process_keeps_other_ids() {
        let mut credentials = FakeCredentials::new(ProcessIds {
            euid: 0,
            ..dropped_ids()
        });

        let error = drop_privileges(&mut credentials, &user()).unwrap_err();

        assert!(error.to_string().contains("after its privilege drop"));
    }

    #[test]
    fn a_drop_stops_at_the_first_failed_call() {
        let mut credentials = FakeCredentials::new(dropped_ids());
        credentials.fail_set_uid = true;

        assert!(drop_privileges(&mut credentials, &user()).is_err());

        assert_eq!(credentials.calls.last().unwrap(), "set_uid 501");
    }

    #[test]
    fn this_test_process_cannot_become_root_through_the_real_calls() {
        if running_as_root() {
            return;
        }
        let mut credentials = ProcessCredentials;
        let before = credentials.ids();

        assert!(credentials.set_uid(0).is_err());
        assert!(credentials.set_gid(0).is_err());
        assert_eq!(credentials.ids(), before);
    }

    #[test]
    fn a_process_that_is_not_root_has_nothing_to_drop() {
        assert_eq!(daemon_uid(501, None, None).unwrap(), None);
        assert_eq!(daemon_uid(501, Some(600), Some("700")).unwrap(), None);
    }

    #[test]
    fn root_runs_as_the_explicit_uid_before_the_sudo_user() {
        assert_eq!(daemon_uid(0, Some(600), Some("700")).unwrap(), Some(600));
        assert_eq!(daemon_uid(0, None, Some("700")).unwrap(), Some(700));
    }

    #[test]
    fn root_with_no_user_to_become_fails_closed() {
        for sudo_uid in [None, Some(""), Some("0"), Some("not-a-number")] {
            let error = daemon_uid(0, None, sudo_uid).unwrap_err();
            assert!(
                error.to_string().contains("Refusing to run"),
                "{sudo_uid:?}"
            );
        }
    }

    #[test]
    fn looking_up_root_or_an_unknown_uid_fails() {
        assert!(UnprivilegedUser::lookup(0).is_err());
        assert!(UnprivilegedUser::lookup(3_999_999_999).is_err());
    }

    #[test]
    fn the_user_database_names_this_test_process() {
        if running_as_root() {
            return;
        }

        let user = UnprivilegedUser::lookup(current_real_uid()).unwrap();

        assert_eq!(user.uid, current_real_uid());
        assert!(!user.name.as_bytes().is_empty());
        assert!(user.home.is_absolute());
    }

    #[test]
    fn a_process_that_is_not_root_leaves_child_commands_unchanged() {
        if running_as_root() {
            return;
        }
        let mut command = Command::new("true");

        run_as_acting_user(&mut command);

        assert_eq!(command.get_envs().count(), 0);
    }
}
