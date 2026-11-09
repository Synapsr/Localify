# A Viewer Can Ask a Target for Access

When a control is unavailable, the viewer's button stays, and clicking it offers what fixes it. The founder asked for this after seeing "runner-mac-01 doesn't allow remote controls" and "Relaunch to share" with nothing to do about either.

Two fixes are needed.

**Asking for access.** A viewer may send `access.request` to ask a target to turn on `control`, or `control` and `backgroundStarts`. The switches still change only on the target, by a person there. The request never changes a setting by itself, and never turns on `share`, which the session already needs.

- **Who decides.** The target's daemon owns the request, and the app shows it in a dialog. The switches change only if the person there clicks `Allow`, and only if nothing changed the settings since the request was recorded. A request that was cancelled, expired or overtaken by a local change is never applied late.
- **What the dialog says.** The switches are machine-wide: turning on `control` lets every paired machine control this one. So the dialog says that, rather than "let A in". It names the asker from the local pin, with its fingerprint, and never from a name the peer chose.
- **When the app is closed.** The target answers that nobody can confirm, and the viewer shows the command to run there. An agent that a person lets run commands on the target, for example over SSH, uses that same command. That keeps the decision with someone who has access to the target, as ADR 0018 does for signing in.
- **Limits.** A request is never queued: it is answered within 2 minutes or it ends. A peer may ask once every 10 minutes, and once an hour after a refusal. A target shows at most 5 dialogs an hour, and the person can stop a machine's requests altogether. So a paired machine cannot flood anyone with dialogs.

**Relaunching a legacy run.** A service started before run PTYs existed has no run id and no stream, so it cannot share its output or be controlled. With `control` on, a viewer may restart it, under the same ownership rules as any restart (ADR 0031). An app run needs the app, and a background run needs `backgroundStarts`. A run in a terminal is never taken over.

- **Naming the run.** The run is named by an identity that survives PID reuse: its boot, its PID and its native start time. That identity is checked before the run is signalled. The executor of the replacement checks it again under the service's lock, so a local launch that came in between wins.
- **What the new run gets.** It has a run id and a stream, and keeps the old run's profile. It uses the service's configured command. A command that a person once typed by hand after `--` is not kept, since a remote control never carries a command. The viewer's confirmation says so.

Both are new session messages, so they take session protocol 2. The version is agreed from the two authenticated `hello`s, so a relay cannot downgrade a session by altering `presence`. Against a target that speaks only version 1, the viewer shows what to run on the target, and no `Ask` button.
