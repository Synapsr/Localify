# Worktree Environment Files Are Explicit

When a Git worktree is attached as a project environment, Localify does not copy environment files from the primary environment automatically. Environment-file creation and copying are explicit actions because they write local configuration files that may contain secrets, may differ intentionally per checkout, and should not overwrite existing worktree files without confirmation. Copy actions should reuse matching relative paths from the primary environment when possible and create missing files only after the user or agent chooses that action.

Copying may happen one file at a time or through a "copy all from primary" flow, but bulk copy must preview which files will be created, skipped, or blocked. Existing target files are skipped by default; overwriting requires an explicit confirmation in the desktop app or an explicit CLI flag.
