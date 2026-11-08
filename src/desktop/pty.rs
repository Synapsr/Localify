//! Pseudo-terminals whose child leads its own session, with the PTY as its
//! controlling terminal, as a terminal emulator starts a shell. A run PTY uses
//! one for `localify dev` (docs/remote-machines.md, "A run PTY").

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

use anyhow::{Context, Result, bail};

use crate::infrastructure::test_pause;

/// The signals a child gets back at their default action, whatever this
/// process does with them: the app ignores or handles some.
const RESET_SIGNALS: [libc::c_int; 15] = [
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGPIPE,
    libc::SIGALRM,
    libc::SIGUSR1,
    libc::SIGUSR2,
    libc::SIGCHLD,
    libc::SIGCONT,
    libc::SIGTSTP,
    libc::SIGTTIN,
    libc::SIGTTOU,
    libc::SIGWINCH,
    libc::SIGURG,
];
/// The most descriptors a child marks close-on-exec one by one, whatever its
/// limit says.
const MAX_MARKED_FD: libc::c_int = 65_536;

/// What a terminal's `c_cc` entry holds to turn its character off.
#[cfg(target_os = "macos")]
const DISABLED: libc::cc_t = libc::_POSIX_VDISABLE;
#[cfg(not(target_os = "macos"))]
const DISABLED: libc::cc_t = 0;

/// A terminal size, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u16,
    pub rows: u16,
}

impl WindowSize {
    /// Refuses a size with no row or no column, which a terminal cannot show.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        if cols == 0 || rows == 0 {
            bail!("Terminal size must include at least one row and column.");
        }
        Ok(Self { cols, rows })
    }

    fn winsize(self) -> libc::winsize {
        libc::winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

/// The parent's side of a PTY: what the child writes is read here, and what is
/// written here is the child's input.
pub struct PtyMaster {
    file: File,
}

impl PtyMaster {
    /// Another handle on the master, for a reader or a writer of its own.
    pub fn try_clone(&self) -> Result<File> {
        self.file
            .try_clone()
            .context("Failed to share the terminal's master")
    }

    /// Tells the terminal, and so its foreground processes, their new size.
    pub fn resize(&self, size: WindowSize) -> Result<()> {
        let winsize = size.winsize();
        // SAFETY: the descriptor is the open master, and TIOCSWINSZ only reads
        // the winsize it is given.
        let result = unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCSWINSZ, &winsize) };
        if result == -1 {
            return Err(io::Error::last_os_error()).context("Failed to resize the terminal");
        }
        Ok(())
    }

    /// The terminal's current settings, as the child's side sees them.
    pub fn termios(&self) -> Result<libc::termios> {
        termios(self.file.as_raw_fd())
    }
}

/// Starts `command` as the leader of a new session whose controlling terminal
/// is a new PTY of `size`, its standard streams on that PTY. With
/// `no_suspend`, the terminal never sends SIGTSTP: Ctrl-Z is then an ordinary
/// character, for a child that no shell could resume.
pub fn spawn(
    mut command: Command,
    size: WindowSize,
    no_suspend: bool,
) -> Result<(PtyMaster, Child)> {
    let (master, slave) = open(size)?;
    if no_suspend {
        disable_suspend(slave.as_raw_fd())?;
    }
    command
        .stdin(slave.try_clone().context("Failed to share the terminal")?)
        .stdout(slave.try_clone().context("Failed to share the terminal")?)
        .stderr(slave);
    let highest = descriptor_limit();
    // SAFETY: the closure runs in the child between fork and exec. It calls
    // only setsid, ioctl, signal, sigemptyset, sigprocmask, fcntl and, on
    // Linux, close_range, all async-signal-safe, allocates nothing, and
    // touches no memory of the parent but `highest`, a copied integer.
    // Standard input is the PTY by then, so TIOCSCTTY makes it the new
    // session's controlling terminal. Every descriptor above standard error
    // is marked close-on-exec rather than closed: it goes at exec, as one
    // another part of the process opened without the flag would, while the
    // pipe that reports a failed exec to the parent keeps working until
    // then. The child starts with every signal at its default action and
    // none blocked.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            for signal in RESET_SIGNALS {
                libc::signal(signal, libc::SIG_DFL);
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            mark_close_on_exec_above_stderr(highest);
            Ok(())
        });
    }
    let child = command
        .spawn()
        .context("Failed to start the terminal's process")?;
    // `command` holds the parent's copies of the slave: dropping it here lets
    // the master read EOF once the child's side closes.
    drop(command);
    Ok((PtyMaster { file: master }, child))
}

/// Marks every descriptor above standard error close-on-exec, in a child
/// between fork and exec: at once where the kernel can, else one by one up
/// to `highest`.
///
/// # Safety
///
/// Only async-signal-safe calls, for a forked child.
unsafe fn mark_close_on_exec_above_stderr(highest: libc::c_int) {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: close_range only changes flags of this process's descriptors.
        let marked = unsafe {
            libc::close_range(
                3,
                libc::c_uint::MAX,
                libc::CLOSE_RANGE_CLOEXEC as libc::c_int,
            )
        };
        if marked == 0 {
            return;
        }
    }
    for fd in 3..highest {
        // SAFETY: F_SETFD on a descriptor changes only its flags; a closed
        // one answers EBADF.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// A new PTY's master and slave, both close-on-exec from their creation, so
/// no child any other thread starts meanwhile can hold them, whether or not
/// it cooperates.
fn open(size: WindowSize) -> Result<(File, File)> {
    let master = open_master()?;
    // SAFETY: the descriptor is the open master.
    let unlocked = unsafe {
        libc::grantpt(master.as_raw_fd()) == 0 && libc::unlockpt(master.as_raw_fd()) == 0
    };
    if !unlocked {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    let name = slave_name(&master)?;
    test_pause::pause_at("pty-allocated");
    // SAFETY: `name` is a NUL-terminated path.
    let slave = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave == -1 {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    // SAFETY: the descriptor was just opened and nothing else owns it.
    let slave = unsafe { File::from_raw_fd(slave) };
    let master = PtyMaster { file: master };
    master.resize(size)?;
    Ok((master.file, slave))
}

/// The master, opened close-on-exec.
fn open_master() -> Result<File> {
    #[cfg(target_os = "linux")]
    // SAFETY: posix_openpt takes flags and returns a new descriptor or -1.
    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
    // macOS's posix_openpt takes no O_CLOEXEC: the device itself does.
    #[cfg(target_os = "macos")]
    // SAFETY: a NUL-terminated path and flags; returns a descriptor or -1.
    let master = unsafe {
        libc::open(
            c"/dev/ptmx".as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if master == -1 {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    // SAFETY: the descriptor was just opened and nothing else owns it.
    Ok(unsafe { File::from_raw_fd(master) })
}

/// The slave's path, from the master.
#[cfg(target_os = "linux")]
fn slave_name(master: &File) -> Result<std::ffi::CString> {
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: ptsname_r writes at most `name.len()` bytes, NUL included.
    if unsafe { libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) } != 0 {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    // SAFETY: ptsname_r wrote a NUL-terminated string into the buffer.
    Ok(unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_owned())
}

/// The slave's path, from the master. macOS has ptsname_r only since 10.13.4
/// and the app supports 10.13: ptsname's static buffer is read under a lock.
#[cfg(target_os = "macos")]
fn slave_name(master: &File) -> Result<std::ffi::CString> {
    static PTSNAME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _reading = PTSNAME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // SAFETY: the descriptor is the open master; ptsname returns a pointer
    // to a static buffer, or null.
    let name = unsafe { libc::ptsname(master.as_raw_fd()) };
    if name.is_null() {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    // SAFETY: non-null, a NUL-terminated string, copied while the lock keeps
    // any other ptsname call of this process from overwriting it.
    Ok(unsafe { std::ffi::CStr::from_ptr(name) }.to_owned())
}

/// The highest descriptor a child marks, from this process's limit.
fn descriptor_limit() -> libc::c_int {
    // SAFETY: rlimit is plain data; getrlimit fills it.
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is valid for the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return MAX_MARKED_FD;
    }
    libc::c_int::try_from(limit.rlim_cur)
        .unwrap_or(MAX_MARKED_FD)
        .clamp(256, MAX_MARKED_FD)
}

fn termios(fd: RawFd) -> Result<libc::termios> {
    // SAFETY: termios is plain data; tcgetattr fills it before it is read.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is an open terminal, and the pointer is valid.
    if unsafe { libc::tcgetattr(fd, &mut settings) } == -1 {
        return Err(io::Error::last_os_error()).context("Failed to read the terminal's settings");
    }
    Ok(settings)
}

/// Turns off the suspend characters: Ctrl-Z (VSUSP), and Ctrl-Y (VDSUSP) on
/// macOS.
fn disable_suspend(fd: RawFd) -> Result<()> {
    let mut settings = termios(fd)?;
    settings.c_cc[libc::VSUSP] = DISABLED;
    #[cfg(target_os = "macos")]
    {
        settings.c_cc[libc::VDSUSP] = DISABLED;
    }
    // SAFETY: the descriptor is an open terminal, and the settings were read
    // from it with only their suspend characters changed.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &settings) } == -1 {
        return Err(io::Error::last_os_error())
            .context("Failed to turn off Ctrl-Z in the terminal");
    }
    Ok(())
}

/// Whether the terminal sends no suspend signal for Ctrl-Z.
pub fn suspend_disabled(settings: &libc::termios) -> bool {
    settings.c_cc[libc::VSUSP] == DISABLED
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn a_child_leads_its_session_on_the_terminal_with_ctrl_z_off() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "stty size; echo \"pid=$$\""]);

        let (master, mut child) = spawn(command, WindowSize::new(91, 17).unwrap(), true).unwrap();

        assert!(suspend_disabled(&master.termios().unwrap()));
        let mut reader = master.try_clone().unwrap();
        let mut output = Vec::new();
        let mut buffer = [0_u8; 1024];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..count]);
            if String::from_utf8_lossy(&output).contains("pid=") && output.ends_with(b"\n") {
                break;
            }
        }
        let output = String::from_utf8_lossy(&output).into_owned();
        assert!(output.contains("17 91"), "{output}");
        child.wait().unwrap();
        let pid: u32 = output
            .lines()
            .find_map(|line| line.trim().strip_prefix("pid="))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(pid, child.id());
    }

    #[test]
    fn writes_reach_the_child_and_resizes_its_terminal() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read line; stty size"]);
        let (master, mut child) = spawn(command, WindowSize::new(80, 24).unwrap(), false).unwrap();

        master.resize(WindowSize::new(100, 30).unwrap()).unwrap();
        master.try_clone().unwrap().write_all(b"go\n").unwrap();

        let mut reader = master.try_clone().unwrap();
        let mut output = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !String::from_utf8_lossy(&output).contains("30 100") {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
            }
        }
        child.wait().unwrap();
        assert!(String::from_utf8_lossy(&output).contains("30 100"));
    }

    #[test]
    fn a_program_that_cannot_be_executed_is_a_spawn_error() {
        // The pipe that reports a failed exec survives the child's setup.
        let spawned = spawn(
            Command::new("/nonexistent/shell"),
            WindowSize::new(80, 24).unwrap(),
            true,
        );

        assert!(spawned.is_err(), "a failed exec read as a spawned terminal");
    }

    #[test]
    fn a_child_holds_no_descriptor_above_standard_error_that_was_not_close_on_exec() {
        // A descriptor this process opened without close-on-exec, as a
        // library or plugin might.
        let (reader, writer) = {
            let mut fds = [0; 2];
            // SAFETY: pipe writes two new descriptors into the array.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            // SAFETY: both were just opened and nothing else owns them.
            unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
        };
        let leaked = writer.as_raw_fd();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!("if [ -e /dev/fd/{leaked} ]; then echo held; else echo none; fi"),
        ]);

        let (master, mut child) = spawn(command, WindowSize::new(80, 24).unwrap(), false).unwrap();

        let mut reader_of_terminal = master.try_clone().unwrap();
        let mut output = Vec::new();
        let mut buffer = [0_u8; 256];
        while !String::from_utf8_lossy(&output).contains("held")
            && !String::from_utf8_lossy(&output).contains("none")
        {
            match reader_of_terminal.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
            }
        }
        child.wait().unwrap();
        drop((reader, writer));
        assert!(
            String::from_utf8_lossy(&output).contains("none"),
            "{}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn a_size_without_rows_or_columns_is_refused() {
        assert!(WindowSize::new(0, 10).is_err());
        assert!(WindowSize::new(10, 0).is_err());
    }
}
