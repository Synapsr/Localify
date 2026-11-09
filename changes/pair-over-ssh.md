---
type: new
link: docs/README.md
---

## Pair two machines over SSH, with nobody at either screen

From a machine that reaches the other over SSH, `localify machines pair <machine>
--over-ssh <destination>` starts the other side through `ssh`, compares both codes
itself, and confirms both only when they match. An agent you gave SSH access to both
machines can now pair them. A pairing cut short after a confirm now says what each
machine saved, instead of claiming nothing was, and pairing again completes it.
