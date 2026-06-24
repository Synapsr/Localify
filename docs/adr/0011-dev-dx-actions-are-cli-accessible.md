# Dev DX Actions Are CLI Accessible

Every project, service, project-environment, and run-session action introduced for Localify's developer workflow must have an explicit non-interactive CLI form. Human terminal flows may offer prompts and shortcuts, but agent and scripted workflows must be able to perform the same operations with flags and receive actionable errors instead of blocking on interactive questions.

Project-owned service domain changes are part of this contract. Agents should use `localify project domain set <service> <domain>` with `--project` and `--env` when needed, rather than editing Localify config files directly.
