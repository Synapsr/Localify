# Bundle and Sync the Localify Agent Skill

Superseded by [0018](0018-agents-use-localify-through-its-mcp-server.md): agents now reach Localify through its MCP server, and there is no skill to bundle or sync.

Localify ships its Agent Skill as part of the Localify app instead of treating GitHub as the runtime source of truth. When the user enables agent skill support, Localify writes the bundled skill to a stable user-local location and delegates installation to the external `skills` CLI so Claude Code, Codex, Cursor, and other supported agents receive the same global symlinked skill. If the setting remains enabled, Localify refreshes that canonical local copy after app updates and reinstalls it so agents stay aligned with the installed Localify version.

This keeps Localify responsible for version synchronization while avoiding a local reimplementation of the multi-agent skill installer.

Agent skill installation is best-effort. Missing Node.js or `npx` must not block Localify installation, daemon setup, DNS, TLS, or project routing; it should surface as an actionable settings state with the command Localify attempted to run.

Even when the setting is enabled by default, Localify requires clear user confirmation before running the global skill installation because the operation writes into agent-owned user directories outside Localify.

By default, Localify installs the Agent Skill only for detected agents. Installing it for every agent supported by the `skills` CLI remains an explicit advanced option.
