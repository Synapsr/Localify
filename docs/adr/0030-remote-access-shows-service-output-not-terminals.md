# Remote Access Shows Service Output, Not Terminals

Version 1 of remote access shows a service's output, read-only, and never a terminal. There is no typing from another machine, no PTY attach and no resize. The panel is labelled "Output" and shows plain, sanitized lines with selectable text, search, and follow or pause. xterm.js may render it read-only, but it receives the same plain lines, never raw bytes. Typing into another machine's shell is remote code execution with extra steps. It would need its own permission model, its own audit and a terminal protocol over the relay, for a need that output and start, stop and restart already cover.

Output must be the service's own, for one run, and its provenance has to come from the channel. Today an app-owned service tab is an interactive shell. The app types the command into it and copies every byte of that shell into the service's output file, prompt and later commands included. Attributing bytes after the fact does not fix that. Markers around a run, or watching which process holds the terminal's foreground, cannot separate service bytes from shell bytes once a service is suspended, a command runs and the service resumes before the reader catches up. So each run gets a dedicated PTY, and the whole-shell file is never exported. This amends ADR 0012: a service tab is now the run, not a shell.

- The app spawns `localify dev` directly as the only child of a new PTY, with no interactive shell and nothing typed into an existing one. Every launch, a restart included, gets a fresh PTY.
- The service tab shows that PTY, interactive and resizable, so the service's colors, prompts, keys and window size behave as before.
- When the run exits, the tab shows the exit code and offers `Restart` and `Open a terminal here`, which opens a separate, ordinary shell tab.
- The run's stream is every byte of that PTY and nothing else. Shell tabs are never written to a stream and never exported.
- A background run's stream is its stdout and stderr, read through pipes.
- `localify logs` reads the same stream, so agents stop reading shell prompts too.
- Until a target's app runs services this way, its app runs report their output as unavailable, never the old file.

The change is visible to everyone who uses the app, remote access or not. Someone who used to type a command into a service tab after the service stopped now opens a shell tab for it. Keystrokes typed into a running service tab are echoed by the terminal into its output, where `localify logs` and paired machines see them. That is accepted and stated: while sharing is on, the tab's footer says that what is typed there appears in the service's output. A nested PTY inside `localify dev` would have kept the old tab, but it would make `localify dev` a terminal multiplexer forwarding resizes, signals and raw input.

Reads take a run id and an offset, read a bounded number of bytes from the segment at that offset, and never read a file to its end. Offsets are logical and never reused across rotation. A read below what is kept says the output was rotated, a run id that is no longer current says the service restarted, and a line longer than 8 KiB arrives in bounded pieces. A follow is flow-controlled by the viewer's acknowledgements, expires without renewal, and ends when the panel closes, so no output streams when nobody reads it.
