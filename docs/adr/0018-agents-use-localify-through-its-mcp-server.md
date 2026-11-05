# Agents Use Localify Through Its MCP Server

Coding agents reach Localify through `localify mcp`, an MCP server over stdio, instead of a bundled skill. A skill only described commands for an agent to type into a shell; an agent without one, or inside a sandbox, could do nothing, and the skill had to be copied and resynced on every update. The MCP server is the installed binary itself, so its tools always match the installed version.

Every manual action of the desktop app and every CLI action an agent can use is a tool, and each tool runs exactly one CLI command with the same binary. The CLI stays the only implementation: a tool adds no behavior, and an action that is missing from the CLI is added there first (ADR 0011). Tool arguments become `--name=value` options, and positional values may not start with `-`, so no argument can turn into a flag.

Some actions are left out on purpose. `uninstall` is for a person to decide. `profile init` is interactive; agents write `localify.toml` directly. Purely visual actions of the app (opening an editor, a browser, or the terminal panel) and its own settings (autostart, updates) have nothing to give an agent.

Long-running commands do not fit a tool call, so `localify dev --detach` runs a service in the background and returns once it listens. Every run of `localify dev` records its PID and start time in the user's state folder, so `services stop` and `services logs` find it from any folder, and a PID that was reused by another process is never signaled.

The server carries its usage rules in its MCP instructions, in place of the skill's text. Localify registers the server with detected agents through their own CLIs, or Cursor's `mcp.json`, from `localify agent install`, from the desktop's Settings, and during setup unless the user turned that off.
