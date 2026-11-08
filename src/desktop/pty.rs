//! Pseudo-terminals whose child leads its own session, with the PTY as its
//! controlling terminal, as a terminal emulator starts a shell. A run PTY uses
//! one for `localify dev` (docs/remote-machines.md, "A run PTY").

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

use anyhow::{Context, Result, bail};

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
    // SAFETY: the closure runs in the child between fork and exec. It calls only
    // setsid and ioctl, both async-signal-safe, and touches no memory of the
    // parent. Standard input is the PTY by then, so TIOCSCTTY makes it the new
    // session's controlling terminal.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
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

fn open(size: WindowSize) -> Result<(File, File)> {
    let (mut master, mut slave): (RawFd, RawFd) = (0, 0);
    let mut winsize = size.winsize();
    // SAFETY: openpty writes two descriptors into the integers. A null name
    // and termios mean the defaults; the winsize is read only.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    };
    if opened != 0 {
        return Err(io::Error::last_os_error()).context("Failed to open a terminal");
    }
    // SAFETY: both descriptors were just opened and nothing else owns them.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    close_on_exec(&master)?;
    close_on_exec(&slave)?;
    Ok((master, slave))
}

/// Keeps a descriptor out of every other child the process starts.
fn close_on_exec(file: &File) -> Result<()> {
    // SAFETY: F_SETFD on a descriptor this process owns changes only its flags.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    if result == -1 {
        return Err(io::Error::last_os_error()).context("Failed to configure the terminal");
    }
    Ok(())
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
    fn a_size_without_rows_or_columns_is_refused() {
        assert!(WindowSize::new(0, 10).is_err());
        assert!(WindowSize::new(10, 0).is_err());
    }
}
