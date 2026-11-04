# No Root Process After Install

On macOS, Localify asks for one administrator approval at install, then nothing of Localify's runs as root. This replaces a root helper and a root daemon, which any process running as the user could turn into root by replacing a file or asking the helper over its socket. launchd, which is already root, holds ports 80 and 443 and hands them to a job running as the user through `launch_activate_socket`. A prototype confirmed this works, that clients enforce a CA limited by name constraints, and that replacing the binary needs no prompt.

The approval runs a shell script of system tools, built by the user's process. Localify's binary never runs as root, and everything root writes is inside the script, so nothing can change between the approval and the write. The CA certificate goes in the script for the same reason, after a check that it carries its name constraint.

A root CA key the user can read is only acceptable because of that constraint. The daemon signs on demand with it and also refuses any name outside the suffix. A CA from an older build has no constraint, so it is replaced rather than trusted again.

The job has `KeepAlive` with `SuccessfulExit` false, on top of the `Sockets` and `RunAtLoad` the prototype used. DNS stays on its unprivileged port, bound by the daemon itself, so once the daemon is gone names stop resolving and no connection ever reaches the sockets to wake it. Without `KeepAlive`, a crash would leave Localify down until the next reboot.

A user cannot drive a system-domain job with `launchctl`, so `stop` is a marker file plus a clean exit. launchd wakes the job on the next connection, which it closes and exits. `restart` is an exit with code 75, which `KeepAlive` turns into a new process on the binary at its path, so updates need no prompt. Commands reach the daemon over its management socket, never a signal to a PID from a file the user can edit.

Trusting a CA needs a confirmation on screen, even as root. It runs last in the script and its result is ignored, so a machine with no screen finishes the install and reports trust as pending, with the exact command to finish it.

Linux keeps its root daemon for now. systemd socket activation is the equivalent and is a separate change.
