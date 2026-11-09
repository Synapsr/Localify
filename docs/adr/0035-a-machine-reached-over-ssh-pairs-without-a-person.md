# A Machine Reached Over SSH Pairs Without a Person

`localify machines pair` used to need a person at a terminal on both machines. The founder wants an agent that already has trusted access to both machines to pair them itself, as one did for him over SSH with a script that typed `yes`.

The short code exists to catch a relay that sits in the middle: it makes both sides derive different codes. What matters is that someone compares the two codes, not that the someone is a person. Someone who can run commands on both machines can already read both codes, so a person adds nothing there. The weak point of the script was elsewhere. It typed `yes` on each side without comparing, so a relay in the middle would have passed.

So `localify machines pair B --over-ssh <destination>` runs the whole pairing from A:
- It starts `localify machines pair --accept --for-ssh` on B through `ssh <destination>`.
- It reads both codes and compares them in Localify's own code.
- It confirms both sides only when they match, and cancels both otherwise.

The trust comes from the SSH access the caller already has, which is exactly the access a person would use, so that access must itself be authenticated. The SSH host key must already be trusted: an unknown or changed key fails, since an attacker who could answer for B over SSH and sit in the relay would otherwise pass the comparison. B's daemon must also report the machine id and the account A expects. Without `--over-ssh`, a person still confirms at a terminal or in the app, as before.

`--for-ssh` is a machine-readable interface for whoever runs it as a user that can reach B's daemon. It is not a check of its own: someone with access to B alone could feed it back its own code, which authorizes B's side only, never A's. The comparison that protects the pairing is A's. It refuses a terminal, which only keeps a person from using it by mistake.

The MCP server still has no pairing tool. An agent pairs through the CLI and its own SSH access, which a person granted. Removing a machine from the account stays a person's decision.

A lost connection after the confirmations started can leave one side pinned. That was already true of every pairing. The contract now says so: the CLI reports what each side did, and pairing the same machines again completes it.
