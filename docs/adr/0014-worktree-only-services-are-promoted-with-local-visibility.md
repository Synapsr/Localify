# Worktree-Only Services Are Promoted With Local Visibility

A service discovered only inside a worktree may be promoted through an explicit action, but promotion makes it a project service rather than an environment-only service. Environment views should show that service where it is present or explicitly configured, while environments that do not contain the service path do not show it in the normal project list. This keeps the project model unified without polluting the primary environment UI with worktree-only additions before they land there.
