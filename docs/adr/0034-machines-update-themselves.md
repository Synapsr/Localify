# Machines Update Themselves

Each Mac with Localify installs new versions by itself, and a machine that sees a paired machine on a newer version checks for it at once. The founder asked for it after updating his laptop to 1.6 and finding his runner still on 1.5, so the new remote controls could not reach it.

**The daemon installs, not the app.** The app's updater only runs while the app is open, and a runner often has it closed. The daemon is always running. It reads the same signed manifest the app reads, downloads the same archive, checks its signature against the public key built into Localify, and replaces the app bundle. A file that fails the signature check is never installed. The daemon already restarts on the new binary when its file changes. Only official releases are installed, never a file a peer or the relay sent.

**Paired machines only trigger a check.** `remote_machines` already knows each machine's version. When a paired machine reports a version newer than this one, this machine checks the manifest now instead of waiting for its next scheduled check. A peer cannot supply a build, pick a version, or force an install: the most it can do is make this machine check the official feed early. That needs no new permission and no consent switch.

**Running services come first.**
- **Background runs** survive the daemon restarting, as they do today.
- **Services in the app's tabs** belong to the app, and they stop when it relaunches. So after installing, the daemon tells the open app that an update is ready. The app shows it and relaunches by itself only once no service runs in its tabs. While a service does, it shows `Restart to update` and waits for the person.
- **The CLI** (`localify` on `PATH`) is the app bundle's binary, so it updates with it.

**On by default, one switch.** Settings → General gets `Install updates automatically`, on for new installs and for existing ones. A person who wants to choose when to update turns it off, and the app's `Check for updates` works as today. With it off, a newer paired machine only makes this one show "Update available".

**macOS only for now.** On Linux, the CLI is a user binary installed by `install.sh`, checked by SHA-256 only. Installing it unattended would need signed Linux artifacts first. Until then, a Linux machine that sees a newer version says "Update available: run the install command again".
