# Machines Update Themselves

A Mac with Localify can install new versions by itself, and a machine that sees a paired machine on a newer version checks for one at once. The founder asked for it after updating his laptop to 1.6 and finding his runner still on 1.5, so the new remote controls could not reach it.

**Who installs.**
- **With the app closed, the daemon installs.** That covers a runner nobody sits at. The daemon already runs as the installing user, not as root, and installs only where that user can replace the app. This feature adds no privileged process.
- **With the app open, the app installs, and only when the person says so.** Its updater downloads in the background and offers `Restart to update`. It never relaunches by itself: a relaunch closes service tabs and shell tabs, and an idle check cannot prove that no one is using them.
- **One installer at a time.** The daemon never replaces a bundle an app is running from. A person dragging a DMG in at the same moment cannot be prevented, and the daemon notices the change and stops.

**Paired machines only trigger a check.** A paired machine's version comes from the relay's presence, which is not authenticated, so it is a hint and nothing more. When it is newer than this machine's, this machine checks the official manifest early, within a machine-wide cooldown. A hint never chooses a build or proves that one exists. Only a release signed with Localify's updater key, whose bundle is signed by Localify's Developer ID, is ever installed.

**Opt-in for existing installs, on for new ones.** Unattended updates restart the daemon, which drops proxied and remote connections for a moment. Someone who installed Localify before this version keeps updating by hand until they turn it on: in Settings, or with `localify update --auto on`, which works over SSH on a runner. New installs start with it on, and the welcome screen says so.

**No automatic rollback in this version.** A recovery that works when the new build cannot even start needs a supervisor outside that build, and this version does not ship one. The installer keeps the previous bundle and documents how to put it back. Linux stays notification-only, since its CLI is checked by SHA-256 and not signed.
