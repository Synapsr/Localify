# Terminal Sessions Are Desktop-Owned

Terminal sessions are real interactive shells owned by the Localify desktop app, rendered in an in-app bottom panel, and scoped to a project environment or one service inside a project environment rather than a generic home-directory shell. Projects group terminal targets but are not terminal targets themselves; project-level actions resolve to the primary environment. The CLI may ask the desktop app to open or focus a terminal session for a project environment, service, or path, but it does not render, multiplex, or persist those sessions itself. This keeps the Codex-style terminal experience in one UI surface while preserving non-interactive CLI access for agent and scripted workflows.

The bottom panel spans the app surface and owns its own right-side terminal sidebar. When the terminal panel is open, the floating bottom navigation is hidden so terminal input, tabs, and resizing are not obstructed.

Desktop service launch actions use the in-app terminal as the only visible terminal surface. External terminal preferences are removed from the service launch workflow because Localify needs a single session owner for focus, stop, and running-state behavior.

If no visible desktop workflow remains for external terminal launch, the external terminal preference is removed from Settings. Existing stored terminal preferences may be ignored during migration rather than surfaced as a dead setting.

The previous combined run preference is also removed from the visible service workflow. Launching a service opens or focuses the integrated terminal session for that service; opening the project in an editor remains a separate explicit action.

The terminal renderer uses xterm.js (`@xterm/xterm` with the fit addon) in the React layer, while Rust owns PTY creation and lifecycle through `portable-pty`. Tauri commands handle session mutations such as create, write, resize, interrupt, and close; terminal output streams from Rust to the frontend through Tauri events or channels. This avoids recreating terminal emulation in React and avoids treating shell processes as ordinary stdout/stderr commands, which would not support interactive programs, ANSI behavior, or terminal resize correctly.

Service actions are terminal-aware but remain service actions. Opening a service terminal starts an interactive shell in the service working directory; launching a service writes the stored structured service command into that PTY and presses enter. If the service target is already reachable, Localify does not relaunch it and exposes stop behavior instead. Stopping a service owned by a Localify terminal session interrupts the foreground command first and keeps the shell alive; stopping an externally running service requires explicit confirmation before Localify kills the process listening on the target port.

Domain surfaces may expose terminal actions only by resolving the domain back to a service or project environment target. Domains are not terminal targets. Tray actions that start or stop domains are removed unless they are redesigned around the service/terminal model, so the product does not expose two competing notions of what can be launched or stopped.

Terminal sessions are runtime state, not project configuration. Localify may persist UI preferences such as panel height or collapsed sidebar state, but it does not persist terminal tabs, terminal history, or terminal output, and terminal contents are not sent to analytics or error reporting. Running service state is derived from Localify-owned session metadata plus target reachability; Localify does not attempt to detect shell prompts because custom prompts make that unreliable.

Terminal output is streamed to the frontend by session id. Rust owns PTY lifecycle and emits bounded output chunks; React writes chunks into the matching xterm instance and keeps only in-memory buffers needed for currently living sessions. Session mutation commands are idempotent where possible so hotkeys, menus, and CLI-triggered desktop actions can all focus the same target without creating duplicate shells.
