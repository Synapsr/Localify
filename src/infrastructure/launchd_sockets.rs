//! Sockets launchd opened for this job, so the daemon can serve ports 80 and 443
//! as a normal user. launchd stays the only root process involved.

use std::io;
use std::net::TcpListener;

/// Socket names in the launchd plist. See `macos_system::plist_contents`.
pub const HTTP_SOCKETS: &str = "HTTP";
pub const HTTPS_SOCKETS: &str = "HTTPS";

/// Takes the listeners launchd holds under `name`.
///
/// `Ok(None)` means this process is not a launchd job with that socket, which is
/// how a foreground or dev daemon learns it has to bind for itself.
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

#[cfg(not(target_os = "macos"))]
fn activate_fds(_name: &str) -> io::Result<Option<Vec<std::os::fd::OwnedFd>>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

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
