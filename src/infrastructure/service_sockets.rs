//! Sockets launchd or systemd opened for this service, so the daemon can serve ports
//! 80 and 443 as a normal user. The service manager stays the only root process.

use std::io;
use std::net::TcpListener;

/// Socket names in the launchd plist and the systemd socket units.
pub const HTTP_SOCKETS: &str = "HTTP";
pub const HTTPS_SOCKETS: &str = "HTTPS";

/// Takes the listeners the service manager holds under `name`.
///
/// `Ok(None)` means this process is not a service with that socket, which is how a
/// foreground or dev daemon learns it has to bind for itself.
///
/// When a port is taken, launchd still hands over a socket for it, never bound.
/// Accepting on one fails at once and forever, so those are dropped here.
pub fn activate(name: &str) -> io::Result<Option<Vec<TcpListener>>> {
    Ok(activate_fds(name)?.map(|fds| {
        fds.into_iter()
            .map(TcpListener::from)
            .filter(is_bound)
            .collect()
    }))
}

fn is_bound(listener: &TcpListener) -> bool {
    listener
        .local_addr()
        .is_ok_and(|address| address.port() != 0)
}

#[cfg(target_os = "macos")]
fn activate_fds(name: &str) -> io::Result<Option<Vec<std::os::fd::OwnedFd>>> {
    use std::ffi::CString;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::raw::{c_char, c_int};

    unsafe extern "C" {
        fn launch_activate_socket(
            name: *const c_char,
            fds: *mut *mut c_int,
            cnt: *mut usize,
        ) -> c_int;
    }

    let name = CString::new(name)?;
    let mut fds: *mut c_int = std::ptr::null_mut();
    let mut count: usize = 0;

    // SAFETY: `name` is a valid NUL-terminated string for the whole call, and `fds`
    // and `count` are valid out-pointers. launch(3) says that on success `fds`
    // points to a heap array of `count` descriptors that the caller frees. launchd
    // hands those descriptors to this process, so each one is open and owned by
    // nothing else, which is what `OwnedFd::from_raw_fd` requires. On failure
    // neither output is read. `fds` is read before it is freed and never again.
    let activated = unsafe {
        match launch_activate_socket(name.as_ptr(), &mut fds, &mut count) {
            0 if fds.is_null() => Ok(Vec::new()),
            0 => {
                let owned = std::slice::from_raw_parts(fds, count)
                    .iter()
                    .map(|&fd| OwnedFd::from_raw_fd(fd))
                    .collect();
                libc::free(fds.cast());
                Ok(owned)
            }
            error => Err(error),
        }
    };

    match activated {
        Ok(fds) => Ok(Some(fds)),
        // ESRCH: not a launchd job. ENOENT: a job without this socket.
        Err(libc::ESRCH | libc::ENOENT) => Ok(None),
        Err(error) => Err(io::Error::from_raw_os_error(error)),
    }
}

#[cfg(target_os = "linux")]
fn activate_fds(name: &str) -> io::Result<Option<Vec<std::os::fd::OwnedFd>>> {
    use std::os::fd::{FromRawFd, OwnedFd};

    let listen_pid = std::env::var("LISTEN_PID").ok();
    let listen_fds = std::env::var("LISTEN_FDS").ok();
    let listen_names = std::env::var("LISTEN_FDNAMES").ok();
    let Some(fds) = systemd_fds_named(
        name,
        std::process::id(),
        listen_pid.as_deref(),
        listen_fds.as_deref(),
        listen_names.as_deref(),
    ) else {
        return Ok(None);
    };

    fds.into_iter()
        .map(|fd| {
            // SAFETY: `LISTEN_PID` names this process, so systemd passed these
            // descriptors to it. Each one is open and owned by nothing else, which is
            // what `OwnedFd::from_raw_fd` requires, and `F_SETFD` only sets a flag on it.
            unsafe {
                let owned = OwnedFd::from_raw_fd(fd);
                // systemd leaves them open across exec, and children must not inherit them.
                if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(owned)
            }
        })
        .collect::<io::Result<Vec<_>>>()
        .map(Some)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn activate_fds(_name: &str) -> io::Result<Option<Vec<std::os::fd::OwnedFd>>> {
    Ok(None)
}

/// The first descriptor systemd passes, per its socket activation protocol.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SD_LISTEN_FDS_START: i32 = 3;

/// The descriptors systemd passed under `name`. `LISTEN_PID` must name this process,
/// because a child inherits the variables but not the descriptors.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_fds_named(
    name: &str,
    own_pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    listen_names: Option<&str>,
) -> Option<Vec<i32>> {
    if listen_pid?.parse::<u32>().ok()? != own_pid {
        return None;
    }
    let count = listen_fds?.parse::<usize>().ok()?;
    let fds: Vec<i32> = listen_names?
        .split(':')
        .take(count)
        .enumerate()
        .filter(|(_, fd_name)| *fd_name == name)
        .map(|(index, _)| SD_LISTEN_FDS_START + index as i32)
        .collect();
    (!fds.is_empty()).then_some(fds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_descriptors_are_matched_by_name_from_three_up() {
        let fds = |name| {
            systemd_fds_named(
                name,
                42,
                Some("42"),
                Some("4"),
                Some("HTTP:HTTP:HTTPS:HTTPS"),
            )
        };

        assert_eq!(fds("HTTP"), Some(vec![3, 4]));
        assert_eq!(fds("HTTPS"), Some(vec![5, 6]));
        assert_eq!(fds("OTHER"), None);
    }

    #[test]
    fn a_process_systemd_did_not_start_has_no_systemd_sockets() {
        let fds = |pid, count, names| systemd_fds_named("HTTP", 42, pid, count, names);

        assert_eq!(fds(None, None, None), None);
        // A child inherits the variables of the process systemd started.
        assert_eq!(fds(Some("7"), Some("1"), Some("HTTP")), None);
        assert_eq!(fds(Some("42"), Some("x"), Some("HTTP")), None);
        assert_eq!(fds(Some("42"), Some("0"), Some("HTTP")), None);
    }

    #[test]
    fn a_socket_launchd_could_not_bind_is_not_a_listener() {
        use std::os::fd::{FromRawFd, OwnedFd};

        // SAFETY: `socket` returns a new descriptor that nothing else owns, or -1,
        // which the assert rejects before `from_raw_fd` sees it.
        let unbound = unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
            assert!(fd >= 0);
            TcpListener::from(OwnedFd::from_raw_fd(fd))
        };
        let bound = TcpListener::bind("127.0.0.1:0").unwrap();

        assert!(!is_bound(&unbound));
        assert!(is_bound(&bound));
    }

    #[test]
    fn a_process_launchd_did_not_start_has_no_sockets() {
        assert!(activate(HTTP_SOCKETS).unwrap().is_none());
    }
}
