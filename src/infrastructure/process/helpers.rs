//! The processes `localify dev` runs before its service: profile CLIs, such
//! as `infisical` and `pass-cli`. They may run in a process group of their
//! own and outlive their caller, so each one is known from its spawn, by
//! identity, and a stop while the profile resolves ends them and what they
//! started.

use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{Identity, Signal, all_processes, process_info, signal_identities, track_tree};

static HELPERS: Mutex<Vec<Identity>> = Mutex::new(Vec::new());
const POLL: Duration = Duration::from_millis(50);

/// A helper this process started, known until it is dropped.
pub struct Helper {
    identity: Option<Identity>,
}

impl Helper {
    /// Knows `child` from now on. Read before anything reaps it, so its PID
    /// is still its own.
    pub fn register(child: &Child) -> Self {
        let identity = process_info(child.id()).map(|process| Identity::of(&process));
        if let Some(identity) = identity {
            known().push(identity);
        }
        Self { identity }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if let Some(identity) = self.identity {
            known().retain(|known| *known != identity);
        }
    }
}

fn known() -> MutexGuard<'static, Vec<Identity>> {
    HELPERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs a helper to its end and returns what it printed, known meanwhile.
pub fn output(command: &mut Command) -> io::Result<Output> {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let _helper = Helper::register(&child);
    child.wait_with_output()
}

/// Ends every known helper and everything it started: SIGTERM, then SIGKILL
/// once `grace` passed, each process checked against its start before each
/// signal. Returns whether none is left.
pub fn end_all(grace: Duration, kill_wait: Duration) -> bool {
    let roots: Vec<Identity> = known().clone();
    let mut tracked = track_tree(&roots, &all_processes());
    signal_identities(&tracked, Signal::Terminate);
    let started = Instant::now();
    let mut killed = false;
    loop {
        tracked = track_tree(&tracked, &all_processes());
        if tracked.is_empty() {
            return true;
        }
        if !killed && started.elapsed() >= grace {
            signal_identities(&tracked, Signal::Kill);
            killed = true;
        }
        if started.elapsed() >= grace + kill_wait {
            return false;
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_helper_that_ignores_sigterm_and_its_child_are_ended() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' TERM HUP; sleep 300 & wait"]);
        let mut child = command.spawn().unwrap();
        let helper = Helper::register(&child);
        let identity = helper.identity.unwrap();
        // Its child has started once the shell waits for it.
        while track_tree(&[identity], &all_processes()).len() < 2 {
            thread::sleep(POLL);
        }
        let tree = track_tree(&[identity], &all_processes());

        assert!(end_all(Duration::from_millis(300), Duration::from_secs(5)));

        child.wait().unwrap();
        assert!(track_tree(&tree, &all_processes()).is_empty());
        drop(helper);
        assert!(!known().contains(&identity));
    }
}
