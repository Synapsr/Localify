# Dev DX Actions Are CLI Accessible

Every project, service, project-environment, and run-session action introduced for Localify's developer workflow must have an explicit non-interactive CLI form. Human terminal flows may offer prompts and shortcuts, but agent and scripted workflows must be able to perform the same operations with flags and receive actionable errors instead of blocking on interactive questions.

Project-owned service domain changes are part of this contract. Agents should use `localify project domain set <service> <domain>` with `--project` and `--env` when needed, rather than editing Localify config files directly.

Worktree discovery and attachment use the `project env` vocabulary instead of a separate `project worktree` command family because Git worktrees are a source of project environments, not a separate Localify concept. The non-interactive surface should include discovery (`project env discover --project <id> --json`), explicit attachment through the existing environment ensure flow (`project env ensure --project <id> --root <path>`), and explicit environment-file copying (`project env copy-env --project <id> --env <slug> --from primary --path <relative-path>`), with overwrite requiring an explicit flag.

Removing a worktree environment follows the same rule: `project env remove <env> --delete-worktree` removes the environment, its domains, its running services and the Git worktree, and `project env prune` removes environments whose folder is gone. Agents call these through the MCP server, whose tools are CLI commands (ADR 0018).
