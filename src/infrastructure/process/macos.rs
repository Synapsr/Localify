//! libproc and sysctl, as Activity Monitor reads them.

use std::mem::size_of;

use libc::{c_int, pid_t};

use super::ProcessInfo;

/// `PROC_ALL_PIDS` from `<sys/proc_info.h>`; the libc crate does not export it.
const PROC_ALL_PIDS: u32 = 1;
/// Room for processes started between sizing the PID list and reading it.
const EXTRA_PIDS: usize = 64;

pub fn pids() -> Vec<u32> {
    // SAFETY: a null buffer of size 0 only asks how many bytes the list needs.
    let needed = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }
    let capacity = needed as usize / size_of::<pid_t>() + EXTRA_PIDS;
    let mut buffer: Vec<pid_t> = vec![0; capacity];
    let Ok(bytes) = c_int::try_from(capacity * size_of::<pid_t>()) else {
        return Vec::new();
    };
    // SAFETY: the buffer is `bytes` bytes of writable, aligned pid_t storage, and
    // the kernel writes at most that many.
    let written =
        unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, buffer.as_mut_ptr().cast(), bytes) };
    if written <= 0 {
        return Vec::new();
    }
    buffer.truncate(written as usize / size_of::<pid_t>());
    buffer
        .into_iter()
        .filter_map(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .collect()
}

pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    let raw = pid_t::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let bsd = bsd_info(raw)?;
    if bsd.pbi_status == libc::SZOMB {
        return None;
    }
    let start = start_abstime(raw)?;
    // SAFETY: getsid takes an integer and touches no memory.
    let sid = unsafe { libc::getsid(raw) };
    Some(ProcessInfo {
        pid,
        ppid: bsd.pbi_ppid,
        pgid: bsd.pbi_pgid,
        sid: u32::try_from(sid).ok(),
        start,
        started_wall: bsd.pbi_start_tvsec,
    })
}

fn bsd_info(pid: pid_t) -> Option<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo holds only integers and byte arrays, so all zeros is valid.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `info` is a writable proc_bsdinfo and `size` is its exact size; the
    // kernel writes at most `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (written == size).then_some(info)
}

/// `ri_proc_start_abstime`: when the process started, in `mach_absolute_time` units.
fn start_abstime(pid: pid_t) -> Option<u64> {
    // SAFETY: rusage_info_v2 holds only integers, so all zeros is valid.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: RUSAGE_INFO_V2 makes the kernel fill exactly one rusage_info_v2, which
    // `info` is. The binding types the buffer as `*mut rusage_info_t`, a pointer to
    // a pointer, but the kernel treats it as the struct's address.
    let result = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        )
    };
    (result == 0 && info.ri_proc_start_abstime > 0).then_some(info.ri_proc_start_abstime)
}

pub fn monotonic_now() -> u64 {
    // The libc crate deprecates its Mach bindings for the mach2 crate; this one
    // call does not justify a dependency.
    #[allow(deprecated)]
    // SAFETY: mach_absolute_time takes no arguments, touches no memory and cannot fail.
    unsafe {
        libc::mach_absolute_time()
    }
}

/// `kern.boottime`, in Unix seconds.
pub fn boot_time() -> Option<u64> {
    let mut boot = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut length = size_of::<libc::timeval>();
    // SAFETY: the name is a NUL-terminated C string, `boot` is a writable timeval
    // and `length` its exact size. No new value is set.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&mut boot as *mut libc::timeval).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || length != size_of::<libc::timeval>() {
        return None;
    }
    u64::try_from(boot.tv_sec)
        .ok()
        .filter(|seconds| *seconds > 0)
}

/// `kern.bootsessionuuid`: a new UUID at every boot.
pub fn boot_id() -> Option<String> {
    let mut buffer = [0_u8; 64];
    let mut length = buffer.len();
    // SAFETY: the name is a NUL-terminated C string, `buffer` is writable for
    // `length` bytes, and sysctl writes back how many it used. No new value is set.
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    let value = buffer.get(..length.min(buffer.len()))?;
    let value = value.split(|byte| *byte == 0).next()?;
    let id = String::from_utf8_lossy(value).trim().to_string();
    (!id.is_empty()).then_some(id)
}
