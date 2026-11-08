# Remote Controls Respect Service Ownership

A paired machine may start, stop and restart a service on a target that allows it, and every control goes through whoever owns the service there. This amends ADR 0012: PTY ownership stays in the desktop app, which remains the only thing that creates, writes to and closes terminals, but the app now accepts start, stop and restart requests from the daemon over a narrow local bridge. The deep link that `localify dev --in-app` uses has no acknowledgement, which is not enough to tell a start that happened from one that did not.

The bridge is a Unix socket that only its owner can open, served by the app while it runs, with one request per connection: hello, launch, stop and restart, each acknowledged once the app has acted on its PTY. The routing follows the owner:

- **App running on the target:** the control goes through the bridge, and the service opens in the app's terminal panel, as its launch button would open it.
- **App absent:** a start runs detached in the background, and only when the target has turned on "allow starts when the app is closed". That is a third switch, separate from sharing and from control, all three off by default.
- **App disappearing mid-dispatch:** the target reconciles. It watches for the run and the port for a few seconds, then reports "done" or "outcome unknown". It never falls back to a background start for a request it handed to the app, which could start the service twice.
- **An app or background run:** stop and restart go through that owner and keep the run's launch profile, as long as it is still permitted.
- **A run in the user's own terminal, or an unknown listener on the port:** the target shows what it observes and refuses the control in v1. Taking over a process someone else holds is not a remote decision.

Controls name explicit ids: machine, project, environment and service, plus a profile name for a start. Each carries the run or config revision the viewer saw, so a control built on stale state is refused as a conflict, and an expiry in the target's clock. The target runs them under a per-service lock shared with every local launch path. A request id is correlation, not exactly-once: a lost acknowledgement means "outcome unknown", followed by a fresh snapshot, and a restart repeated after a reconnect fails because the run it expected is gone. A control never carries a command, a path, an env file or value, an option that creates a project, or a dangerous launch profile. The target checks its own settings and a fresh authorization lease before each one.
