---
type: new
link: docs/README.md#cleaning-up-worktrees
---

## Remove a worktree and its domains when the work is done

`localify project env remove <env> --delete-worktree` stops the worktree's services,
deletes its folder, and unregisters its domains, so finished branches leave nothing
behind. If the worktree has uncommitted changes, nothing is removed. `localify project
env prune` cleans up after worktrees you deleted another way.
