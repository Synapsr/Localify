//! The terminal panel's sessions live in `localify::desktop::terminal`; this
//! is where they reach the window, as Tauri events.

use localify::desktop::shell_environment::ShellEnvironmentDto;
use localify::desktop::terminal::{
    TerminalEvents, TerminalExitPayload, TerminalOutputPayload, TerminalSessionDto,
};
use tauri::{AppHandle, Emitter, Runtime};

pub const TERMINAL_OUTPUT_EVENT: &str = "terminal-output";
pub const TERMINAL_EXIT_EVENT: &str = "terminal-exit";
/// A session changed: a service tab's run ended and was finalized.
pub const TERMINAL_SESSION_EVENT: &str = "terminal-session";
pub const SHELL_ENVIRONMENT_EVENT: &str = "shell-environment";

/// The window's side of the terminal panel's events.
pub struct WindowEvents<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> TerminalEvents for WindowEvents<R> {
    fn output(&self, payload: TerminalOutputPayload) {
        let _ = self.0.emit(TERMINAL_OUTPUT_EVENT, payload);
    }

    fn exit(&self, payload: TerminalExitPayload) {
        let _ = self.0.emit(TERMINAL_EXIT_EVENT, payload);
    }

    fn session(&self, session: TerminalSessionDto) {
        let _ = self.0.emit(TERMINAL_SESSION_EVENT, session);
    }

    fn shell_environment(&self, status: ShellEnvironmentDto) {
        let _ = self.0.emit(SHELL_ENVIRONMENT_EVENT, status);
    }
}
