# A Viewer Can Ask a Target for Access

When a control is unavailable, the viewer's button stays, and clicking it offers what fixes it. The founder asked for this after seeing "runner-mac-01 doesn't allow remote controls" and "Relaunch to share" with nothing to do about either.

Two fixes are needed.

**Asking for access.** A viewer may send `access.request` to ask a target to turn on `control`, or `control` and `backgroundStarts`. The switches still change only on the target, by a person there (see "Target settings"). The request never changes a setting by itself. When the desktop app runs on the target, it shows the request in a dialog, and the switch changes only if the person there clicks `Allow`. When the app is absent, the target answers that nobody can confirm, and the viewer shows the command to run there. An agent that a person lets run commands on the target, for example over SSH, uses that same command. That keeps the decision with someone who has access to the target, as ADR 0018 does for signing in. A request is never queued for later: it is answered within 2 minutes or it ends. A target holds one request at a time, and a viewer may ask a target once a minute, so a paired machine cannot flood a person with dialogs.

**Relaunching a legacy run.** A service started before run PTYs exists has no run id and no stream, so it cannot share its output or be controlled. With `control` on, a viewer may restart it. The target stops it the way `localify stop` stops it today, which only signals a PID whose identity is proven (ADR 0031's rule for legacy runs). It then starts the service again under "Who acts": in the app when the app runs, in the background when `backgroundStarts` allows it. The new run has a run id and a stream. The relaunch keeps the run's profile and uses the service's configured command. A command that a person once typed by hand after `--` is not kept, since a remote control never carries a command. The viewer's confirmation says so.

Both are new session messages, so they take session protocol version 2. Against a target that speaks only version 1, the viewer still shows what to run on the target, without the `Ask` button.
