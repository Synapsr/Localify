//! The shell environment runs start with, resolved once per app session
//! (docs/remote-machines.md, "The shell environment").

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use crate::infrastructure::shell_env::{self, Environment};

/// Whether the user's shell environment was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEnvironmentDto {
    pub state: ShellEnvironmentState,
    /// The shell that was asked.
    pub shell: String,
    /// Why it could not be read.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellEnvironmentState {
    Resolving,
    Ready,
    /// Runs start with the app's own environment.
    Failed,
}

enum Resolved {
    Pending,
    Ready(Environment),
    Failed(String),
}

/// The user's login-shell environment, read the first time a run needs it
/// and kept until `reload`.
pub struct ShellEnvironment {
    executable: PathBuf,
    shell: PathBuf,
    /// Held while the shell is asked, so concurrent launches wait for one read.
    resolving: Mutex<()>,
    resolved: Mutex<Resolved>,
}

impl ShellEnvironment {
    /// `executable` is the pinned `localify`, which prints the environment;
    /// `shell` the user's login shell.
    pub fn new(executable: PathBuf, shell: PathBuf) -> Self {
        Self {
            executable,
            shell,
            resolving: Mutex::new(()),
            resolved: Mutex::new(Resolved::Pending),
        }
    }

    /// The environment a run starts with: the shell's, or the app's own when
    /// the shell's could not be read. Waits for a read in progress.
    pub fn environment(&self) -> Environment {
        let _resolving = lock(&self.resolving);
        if matches!(*lock(&self.resolved), Resolved::Pending) {
            self.resolve();
        }
        match &*lock(&self.resolved) {
            Resolved::Ready(environment) => environment.clone(),
            Resolved::Pending | Resolved::Failed(_) => std::env::vars_os().collect(),
        }
    }

    /// Reads the shell environment again, for `Reload shell environment`.
    pub fn reload(&self) -> ShellEnvironmentDto {
        let _resolving = lock(&self.resolving);
        *lock(&self.resolved) = Resolved::Pending;
        self.resolve();
        self.status()
    }

    pub fn status(&self) -> ShellEnvironmentDto {
        let (state, reason) = match &*lock(&self.resolved) {
            Resolved::Pending => (ShellEnvironmentState::Resolving, None),
            Resolved::Ready(_) => (ShellEnvironmentState::Ready, None),
            Resolved::Failed(reason) => (ShellEnvironmentState::Failed, Some(reason.clone())),
        };
        ShellEnvironmentDto {
            state,
            shell: self.shell.display().to_string(),
            reason,
        }
    }

    fn resolve(&self) {
        let resolved = match shell_env::resolve(&self.executable, &self.shell) {
            Ok(environment) if has_path(&environment) => Resolved::Ready(environment),
            Ok(_) => Resolved::Failed("Your shell's environment has no PATH.".into()),
            Err(error) => Resolved::Failed(format!("{error:#}")),
        };
        *lock(&self.resolved) = resolved;
    }
}

fn has_path(environment: &[(OsString, OsString)]) -> bool {
    environment.iter().any(|(key, _)| key == "PATH")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_that_cannot_run_leaves_runs_on_the_app_s_environment() {
        let shell = ShellEnvironment::new(
            PathBuf::from("/nonexistent/localify"),
            PathBuf::from("/bin/sh"),
        );
        assert_eq!(shell.status().state, ShellEnvironmentState::Resolving);

        let environment = shell.environment();

        let status = shell.status();
        assert_eq!(status.state, ShellEnvironmentState::Failed);
        assert!(status.reason.is_some());
        assert_eq!(environment, std::env::vars_os().collect::<Vec<_>>());
    }
}
