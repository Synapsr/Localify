//! The user's interactive login-shell environment, read outside any terminal a
//! run's output is captured from (docs/remote-machines.md, "The shell
//! environment").
//!
//! A run PTY has no shell in it, so a service would lose what shell init sets
//! up: `PATH` entries, `nvm`, `asdf`, profile CLIs. The resolver runs the
//! user's shell once, as editors do, and has it run `localify shell-env
//! <nonce>`, which prints the environment framed by two copies of the nonce.
//! Whatever the shell's startup files print outside the frame is ignored;
//! anything malformed inside it, a frame printed twice, more than 1 MiB, or
//! more than the deadline is a failure, and the caller runs services with its
//! own environment instead.

use std::collections::HashSet;
use std::ffi::{CStr, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// One deadline for the spawn, the reading and the cleanup.
pub const DEADLINE: Duration = Duration::from_secs(10);
/// The most the resolver reads from the shell, framing and noise included.
pub const MAX_CAPTURE: usize = 1024 * 1024;
/// Kept out of the deadline for killing and reaping the shell.
const CLEANUP: Duration = Duration::from_millis(500);
/// How long the output may stay quiet once the shell exited, for what is
/// still in the pipe to arrive.
const LINGER: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(20);
const NONCE_BYTES: usize = 16;

/// Variables as the user's shell init leaves them, in the order it printed.
pub type Environment = Vec<(OsString, OsString)>;

/// `localify shell-env <nonce>`: prints this process's environment, framed.
/// Refuses a nonce the resolver could not have made, so the command cannot
/// be used to print anything else.
pub fn print_environment(nonce: &str) -> Result<()> {
    if !valid_nonce(nonce) {
        bail!(
            "'{nonce}' is not a nonce: expected {} hex digits.",
            NONCE_BYTES * 2
        );
    }
    let frame = frame(nonce, std::env::vars_os());
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&frame)
        .and_then(|()| stdout.flush())
        .context("Failed to print the environment")
}

/// The nonce, then each variable as `KEY=VALUE`, all separated by NUL bytes,
/// then the nonce again.
fn frame(nonce: &str, variables: impl Iterator<Item = (OsString, OsString)>) -> Vec<u8> {
    let mut frame = nonce.as_bytes().to_vec();
    for (key, value) in variables {
        frame.push(0);
        frame.extend_from_slice(key.as_bytes());
        frame.push(b'=');
        frame.extend_from_slice(value.as_bytes());
    }
    frame.push(0);
    frame.extend_from_slice(nonce.as_bytes());
    frame
}

fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == NONCE_BYTES * 2
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Reads the environment the user's login shell sets up, with `localify` the
/// pinned executable that prints it. `shell` is `$SHELL`, or the account's
/// shell when unset. Never waits longer than [`DEADLINE`].
pub fn resolve(localify: &Path, shell: &Path) -> Result<Environment> {
    resolve_within(localify, shell, DEADLINE)
}

/// [`resolve`] with another deadline, for tests.
pub fn resolve_within(localify: &Path, shell: &Path, deadline: Duration) -> Result<Environment> {
    let started = Instant::now();
    let nonce = new_nonce()?;
    let (program, args) = invocation(shell, localify, &nonce)?;
    let mut command = Command::new(&program);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: the closure runs in the child between fork and exec and only
    // calls setsid, which is async-signal-safe and touches no memory. Its own
    // session has no controlling terminal, and its process group is what the
    // deadline kills.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start {}", program.display()))?;
    let Some(stdout) = child.stdout.take() else {
        kill_group(&mut child);
        bail!("Failed to read the output of {}", program.display());
    };
    let until = started + deadline.saturating_sub(CLEANUP);
    let outcome = capture(&child, stdout, until).and_then(|captured| parse(&captured, &nonce));
    match &outcome {
        Ok(_) => reap(&mut child, started + deadline),
        Err(_) => kill_group(&mut child),
    }
    outcome
}

/// Reads what the shell prints until it closes its output, or until it has
/// exited and nothing more comes: a process its startup files left running
/// may hold the output open. Reading on to the end, rather than stopping at
/// the first complete frame, is what shows a frame printed twice. Fails past
/// `until` or [`MAX_CAPTURE`] bytes.
fn capture(child: &Child, stdout: impl Read + Send + 'static, until: Instant) -> Result<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || read_chunks(stdout, &sender));
    let mut captured = Vec::new();
    let mut quiet_since: Option<Instant> = None;
    loop {
        match receiver.recv_timeout(POLL) {
            Ok(Ok(chunk)) => {
                captured.extend_from_slice(&chunk);
                quiet_since = None;
            }
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(captured),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if exited_unreaped(child.id()) {
                    let since = *quiet_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= LINGER {
                        return Ok(captured);
                    }
                }
            }
        }
        if captured.len() > MAX_CAPTURE {
            bail!(
                "Your shell printed more than {} MiB while starting.",
                MAX_CAPTURE / (1024 * 1024)
            );
        }
        if Instant::now() >= until {
            bail!("Your shell's startup files did not finish in time.");
        }
    }
}

/// Sends what the shell prints, chunk by chunk, until it closes its output.
fn read_chunks(mut stdout: impl Read, sender: &mpsc::Sender<Result<Vec<u8>>>) {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let chunk = match stdout.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => Ok(buffer[..count].to_vec()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                Err(anyhow::Error::new(error).context("Failed to read the shell's output"))
            }
        };
        let failed = chunk.is_err();
        if sender.send(chunk).is_err() || failed {
            return;
        }
    }
}

/// Whether the process exited, without reaping it: its PID, and so the id of
/// the process group it leads, stay reserved until the caller reaps it.
fn exited_unreaped(pid: libc::id_t) -> bool {
    // SAFETY: siginfo_t is plain data, zeroed before waitid fills it.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is valid; WNOWAIT leaves the child to be reaped
    // later, and WNOHANG returns at once.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    // SAFETY: waitid succeeded, so si_pid is set: zero while the child runs.
    result == 0 && unsafe { info.si_pid() } != 0
}

/// The variables between the two copies of the nonce. The frame must appear
/// exactly once, and hold nothing but `KEY=VALUE` entries, each key once.
pub fn parse(captured: &[u8], nonce: &str) -> Result<Environment> {
    let nonce = nonce.as_bytes();
    match occurrences(captured, nonce) {
        0 | 1 => bail!("The shell exited before printing its environment."),
        2 => {}
        _ => bail!("The shell printed its environment's frame more than once."),
    }
    let Some(open) = find(captured, nonce) else {
        bail!("The shell exited before printing its environment.");
    };
    let start = open + nonce.len();
    let Some(close) = find(&captured[start..], nonce).map(|index| start + index) else {
        bail!("The shell exited before printing its environment.");
    };
    let body = &captured[start..close];
    // `\0K=V\0K=V\0`, or `\0` alone for an empty environment.
    let Some(entries) = body.strip_prefix(b"\0") else {
        bail!("Something printed inside the shell's environment.");
    };
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let Some(entries) = entries.strip_suffix(b"\0") else {
        bail!("Something printed inside the shell's environment.");
    };
    let mut seen = HashSet::new();
    let mut variables = Vec::new();
    for entry in entries.split(|byte| *byte == 0) {
        let Some(equals) = entry.iter().position(|byte| *byte == b'=') else {
            bail!("Something printed inside the shell's environment.");
        };
        let (key, value) = (&entry[..equals], &entry[equals + 1..]);
        if key.is_empty() || !seen.insert(key) {
            bail!("Something printed inside the shell's environment.");
        }
        variables.push((
            OsStr::from_bytes(key).to_os_string(),
            OsStr::from_bytes(value).to_os_string(),
        ));
    }
    Ok(variables)
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// How each shell runs `"<localify>" shell-env <nonce>` after reading its
/// interactive login startup files. A shell Localify does not know runs as
/// `/bin/sh -lc`, which reads only the POSIX profile files.
fn invocation(shell: &Path, localify: &Path, nonce: &str) -> Result<(PathBuf, Vec<OsString>)> {
    let Some(path) = localify.to_str() else {
        bail!(
            "'{}' is not valid UTF-8, so no shell can run it.",
            localify.display()
        );
    };
    let name = shell
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let (program, args) = match name {
        "zsh" | "bash" => (
            shell.to_path_buf(),
            vec![
                "-ilc".into(),
                format!("{} shell-env {nonce}", posix_quote(path)),
            ],
        ),
        "fish" => (
            shell.to_path_buf(),
            vec![
                "-ilc".into(),
                format!("{} shell-env {nonce}", fish_quote(path)),
            ],
        ),
        "nu" => (
            shell.to_path_buf(),
            vec![
                "-l".into(),
                "-i".into(),
                "-c".into(),
                format!("^{} shell-env {nonce}", nu_quote(path)),
            ],
        ),
        _ => (
            PathBuf::from("/bin/sh"),
            vec![
                "-lc".into(),
                format!("{} shell-env {nonce}", posix_quote(path)),
            ],
        ),
    };
    Ok((program, args.into_iter().map(OsString::from).collect()))
}

fn posix_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Inside fish's single quotes, only `\\` and `\'` are escapes.
fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Nushell's double-quoted string, whose escapes start with a backslash.
fn nu_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn new_nonce() -> Result<String> {
    let mut bytes = [0_u8; NONCE_BYTES];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context("Failed to read random bytes from /dev/urandom")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Kills every process of the resolver's session, then reaps the shell. The
/// shell is not reaped before, so its PID, the group's id, is still reserved.
fn kill_group(child: &mut Child) {
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill with a negative PID only sends a signal; the group is
        // the shell's own, and the unreaped shell keeps its id reserved.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.wait();
}

/// Waits for the shell to exit after it printed its environment, and ends
/// its session if it does not within what is left of the deadline.
fn reap(child: &mut Child, until: Instant) {
    while Instant::now() < until {
        if exited_unreaped(child.id()) {
            let _ = child.wait();
            return;
        }
        thread::sleep(POLL);
    }
    kill_group(child);
}

/// `$SHELL`, else the account's login shell, else `/bin/sh`: an app started
/// from the Dock may have no `$SHELL`.
pub fn user_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .map(PathBuf::from)
        .or_else(account_shell)
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

fn account_shell() -> Option<PathBuf> {
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: passwd is plain data; getpwuid_r fills it before it is read.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and the buffer outlives the
    // strings getpwuid_r points into it.
    let found = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if found != 0 || result.is_null() || entry.pw_shell.is_null() {
        return None;
    }
    // SAFETY: getpwuid_r succeeded, so pw_shell is a NUL-terminated string in
    // `buffer`, which is still alive.
    let shell = unsafe { CStr::from_ptr(entry.pw_shell) };
    let shell = OsString::from_vec(shell.to_bytes().to_vec());
    (!shell.is_empty()).then(|| PathBuf::from(shell))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "00112233445566778899aabbccddeeff";

    fn variables(pairs: &[(&str, &str)]) -> impl Iterator<Item = (OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn a_frame_parses_back_to_its_variables() {
        let frame = frame(
            NONCE,
            variables(&[("PATH", "/a:/b"), ("EMPTY", ""), ("EQ", "a=b\nc")]),
        );

        let parsed = parse(&frame, NONCE).unwrap();

        assert_eq!(
            parsed,
            variables(&[("PATH", "/a:/b"), ("EMPTY", ""), ("EQ", "a=b\nc")]).collect::<Vec<_>>()
        );
    }

    #[test]
    fn what_the_shell_prints_outside_the_frame_is_ignored() {
        let mut captured = b"Welcome!\n\x00stray=1\x00".to_vec();
        captured.extend(frame(NONCE, variables(&[("A", "1")])));
        captured.extend(b"bye\n");

        assert_eq!(parse(&captured, NONCE).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_environment_is_a_frame_too() {
        assert!(
            parse(&frame(NONCE, std::iter::empty()), NONCE)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn text_inside_the_frame_is_a_failure() {
        let mut captured = NONCE.as_bytes().to_vec();
        captured.extend(b"\0A=1\0injected\0B=2\0");
        captured.extend(NONCE.as_bytes());

        assert!(parse(&captured, NONCE).is_err());
    }

    #[test]
    fn a_frame_printed_twice_is_a_failure() {
        let mut captured = frame(NONCE, variables(&[("PATH", "/evil")]));
        captured.extend(frame(NONCE, variables(&[("PATH", "/bin")])));

        assert!(parse(&captured, NONCE).is_err());
    }

    #[test]
    fn a_repeated_key_or_an_empty_one_is_a_failure() {
        assert!(parse(&frame(NONCE, variables(&[("A", "1"), ("A", "2")])), NONCE).is_err());
        assert!(parse(&frame(NONCE, variables(&[("", "1")])), NONCE).is_err());
    }

    #[test]
    fn a_missing_end_is_a_failure() {
        let mut captured = NONCE.as_bytes().to_vec();
        captured.extend(b"\0A=1");

        assert!(parse(&captured, NONCE).is_err());
    }

    #[test]
    fn a_forged_nonce_frames_nothing() {
        let forged = frame(
            "ffeeddccbbaa99887766554433221100",
            variables(&[("PATH", "/evil")]),
        );

        assert!(parse(&forged, NONCE).is_err());
    }

    #[test]
    fn only_a_nonce_the_resolver_makes_is_printed() {
        assert!(print_environment("../../etc").is_err());
        assert!(print_environment("00112233445566778899AABBCCDDEEFF").is_err());
        assert!(valid_nonce(&new_nonce().unwrap()));
    }

    #[test]
    fn each_shell_runs_the_command_its_own_way() {
        let localify = Path::new("/Apps/It's \"Localify\"/localify");
        let (program, args) = invocation(Path::new("/bin/zsh"), localify, NONCE).unwrap();
        assert_eq!(program, Path::new("/bin/zsh"));
        assert_eq!(
            args,
            [
                "-ilc",
                &format!("'/Apps/It'\"'\"'s \"Localify\"/localify' shell-env {NONCE}")
            ]
        );

        let (_, args) = invocation(Path::new("/opt/fish"), localify, NONCE).unwrap();
        assert_eq!(
            args[1].to_str().unwrap(),
            format!("'/Apps/It\\'s \"Localify\"/localify' shell-env {NONCE}")
        );

        let (_, args) = invocation(Path::new("/opt/nu"), localify, NONCE).unwrap();
        assert_eq!(args[..3], ["-l", "-i", "-c"]);
        assert_eq!(
            args[3].to_str().unwrap(),
            format!("^\"/Apps/It's \\\"Localify\\\"/localify\" shell-env {NONCE}")
        );

        let (program, args) = invocation(Path::new("/usr/bin/tcsh"), localify, NONCE).unwrap();
        assert_eq!(program, Path::new("/bin/sh"));
        assert_eq!(args[0], "-lc");
    }
}
