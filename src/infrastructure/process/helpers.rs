//! The processes `localify dev` runs before its service: profile CLIs, such
//! as `infisical` and `pass-cli`. They may run in a process group of their
//! own and outlive their caller, so each one is known from its spawn, by
//! identity, and a stop while the profile resolves ends them and what they
//! started.
//!
//! A helper is spawned and registered under one lock, which a stop takes to
//! cancel: a stop never misses a helper that is starting, and no helper
//! starts after it.

use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{Identity, Signal, all_processes, process_info, signal_identities, track_tree};

const POLL: Duration = Duration::from_millis(50);

/// The helpers of one process; `localify dev` has one, [`helpers`].
pub struct Helpers {
    registry: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    known: Vec<Identity>,
    /// A stop ended the resolution: no helper starts any more.
    cancelled: bool,
}

static HELPERS: Helpers = Helpers::new();

/// This process's helpers.
pub fn helpers() -> &'static Helpers {
    &HELPERS
}

/// A helper this process started, known until it is dropped.
pub struct Helper<'a> {
    helpers: &'a Helpers,
    identity: Option<Identity>,
}

impl Drop for Helper<'_> {
    fn drop(&mut self) {
        if let Some(identity) = self.identity {
            self.helpers.lock().known.retain(|known| *known != identity);
        }
    }
}

impl Helpers {
    pub const fn new() -> Self {
        Self {
            registry: Mutex::new(Registry {
                known: Vec::new(),
                cancelled: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a helper, known from then on. Refused once a stop cancelled
    /// the resolution, so nothing starts after it.
    pub fn spawn(&self, command: &mut Command) -> io::Result<(Child, Helper<'_>)> {
        self.spawn_with(command, || {})
    }

    /// `spawn`, with `between` run after the spawn and before the
    /// registration, still under the lock: where a test holds it.
    fn spawn_with(
        &self,
        command: &mut Command,
        between: impl FnOnce(),
    ) -> io::Result<(Child, Helper<'_>)> {
        let mut registry = self.lock();
        if registry.cancelled {
            return Err(io::Error::other(
                "the launch was stopped while its profile resolved",
            ));
        }
        let child = command.spawn()?;
        between();
        // Read before anything reaps it, so its PID is still its own.
        let identity = process_info(child.id()).map(|process| Identity::of(&process));
        if let Some(identity) = identity {
            registry.known.push(identity);
        }
        drop(registry);
        Ok((
            child,
            Helper {
                helpers: self,
                identity,
            },
        ))
    }

    /// Runs a helper to its end and returns what it printed, known meanwhile.
    pub fn output(&self, command: &mut Command) -> io::Result<Output> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let (child, _helper) = self.spawn(command)?;
        child.wait_with_output()
    }

    /// Cancels the resolution, so no helper starts any more, then ends every
    /// known helper and everything it started: SIGTERM, then SIGKILL once
    /// `grace` passed, each process checked against its start before each
    /// signal. Returns whether none is left.
    pub fn end_all(&self, grace: Duration, kill_wait: Duration) -> bool {
        let roots = {
            let mut registry = self.lock();
            registry.cancelled = true;
            registry.known.clone()
        };
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
}

impl Default for Helpers {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// A helper that ignores SIGTERM and SIGHUP, as a CLI waiting for a login
    /// may.
    fn stubborn() -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' TERM HUP; exec sleep 300"]);
        command
    }

    #[test]
    fn a_stop_while_a_helper_starts_ends_it_once_it_is_known() {
        let helpers = Helpers::new();
        let (spawned, wait_spawned) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel::<()>();

        thread::scope(|scope| {
            let helpers = &helpers;
            let starting = scope.spawn(move || {
                helpers
                    .spawn_with(&mut stubborn(), || {
                        spawned.send(()).unwrap();
                        wait_resume.recv().unwrap();
                    })
                    .map(|(mut child, helper)| {
                        let identity = helper.identity;
                        let _ = child.wait();
                        identity
                    })
            });
            // Spawned, not registered yet: the stop waits for it.
            wait_spawned.recv().unwrap();
            let stopping =
                scope.spawn(|| helpers.end_all(Duration::from_millis(200), Duration::from_secs(5)));
            resume.send(()).unwrap();

            assert!(stopping.join().unwrap(), "the helper outlived the stop");
            let identity = starting.join().unwrap().unwrap().unwrap();
            assert!(track_tree(&[identity], &all_processes()).is_empty());
        });
    }

    #[test]
    fn no_helper_starts_once_the_resolution_is_cancelled() {
        let helpers = Helpers::new();
        helpers.end_all(Duration::from_millis(10), Duration::from_millis(10));

        let refused = helpers.spawn(&mut stubborn());

        assert!(refused.is_err(), "a helper started after the stop");
    }

    #[test]
    fn parallel_lookups_during_a_stop_leave_no_helper_alive() {
        let helpers = Helpers::new();
        let (started, wait_started) = mpsc::channel();

        thread::scope(|scope| {
            let lookups: Vec<_> = (0..6)
                .map(|_| {
                    let started = started.clone();
                    let helpers = &helpers;
                    scope.spawn(move || {
                        let _ = started.send(());
                        helpers.output(&mut stubborn())
                    })
                })
                .collect();
            wait_started.recv().unwrap();

            assert!(helpers.end_all(Duration::from_millis(200), Duration::from_secs(5)));

            for lookup in lookups {
                // Each one was ended by the stop, or refused after it.
                match lookup.join().unwrap() {
                    Ok(output) => assert!(!output.status.success()),
                    Err(error) => assert!(error.to_string().contains("stopped"), "{error}"),
                }
            }
        });
        assert!(helpers.lock().known.is_empty());
    }
}
