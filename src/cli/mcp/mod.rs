//! `localify mcp`: an MCP server over stdio that gives coding agents every
//! Localify action. Each tool runs one CLI command with this same binary, so the
//! CLI stays the only implementation and agents see exactly what a person sees.
//!
//! The protocol is JSON-RPC 2.0, one message per line. Requests are answered in
//! order; nothing but protocol messages is written to stdout.

mod tools;

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::application::cloud_errors::RAISE_LIMIT;
use crate::branding::BRANDING;
use tools::{TOOLS, Tool, ToolArgs};

const INSTRUCTIONS: &str = include_str!("instructions.md");

/// Newest first; the first one is answered when the client asks for another.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const PARSE_ERROR: i64 = -32700;

pub fn serve(config_path: &Path) -> Result<()> {
    let runner = CliRunner {
        executable: std::env::current_exe().with_context(|| {
            format!(
                "Failed to resolve the {} executable path",
                BRANDING.cli_command()
            )
        })?,
        // A relative path would be read from the `cwd` each tool runs in.
        config_path: std::path::absolute(config_path).with_context(|| {
            format!(
                "Failed to resolve the config path '{}'",
                config_path.display()
            )
        })?,
    };
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.context("Failed to read from stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&line, &runner) {
            writeln!(stdout, "{response}").context("Failed to write to stdout")?;
            stdout.flush().context("Failed to write to stdout")?;
        }
    }
    Ok(())
}

/// The response to one message, or `None` for a notification.
fn handle_line(line: &str, runner: &CliRunner) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => return Some(error_response(Value::Null, PARSE_ERROR, &error.to_string())),
    };
    let id = message.get("id").cloned()?;
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];

    let result = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => {
            Ok(json!({ "tools": TOOLS.iter().map(Tool::definition).collect::<Vec<_>>() }))
        }
        "tools/call" => call_tool(params, runner),
        _ => {
            return Some(error_response(
                id,
                METHOD_NOT_FOUND,
                &format!("Unknown method '{method}'"),
            ));
        }
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => error_response(id, INVALID_PARAMS, &format!("{error:#}")),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params["protocolVersion"].as_str().unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|version| **version == requested)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": BRANDING.cli_command(),
            "title": BRANDING.app_name,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// Runs a tool. A command that fails is a tool result with `isError`, so the
/// agent reads Localify's message; only a call that names no tool is an error.
fn call_tool(params: &Value, runner: &CliRunner) -> Result<Value> {
    let name = params["name"]
        .as_str()
        .context("tools/call needs a tool name")?;
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .with_context(|| format!("Unknown tool '{name}'"))?;
    let args = ToolArgs::new(&params["arguments"])?;

    let (text, failed) = match prepare(tool, &args) {
        Ok((argv, cwd)) => runner.run(&argv, cwd.as_deref()),
        Err(error) => (format!("{error:#}"), true),
    };
    let text = if failed { for_an_agent(text) } else { text };
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": failed,
    }))
}

/// Paying is for a person (ADR 0018): when a command says the plan is
/// needed, the agent learns what it costs and to ask the user; when a limit
/// can be raised, the command that raises it stays in the text, for the
/// agent to suggest.
fn for_an_agent(text: String) -> String {
    if let Some(hint) = remote_hint(&text) {
        return format!("{text}\n\n{hint}");
    }
    if text.contains(RAISE_LIMIT) {
        return format!(
            "{text}\n\nRaising the limit is a purchase, so it is the user's call: suggest they run that command themselves, then try again. Do not start a checkout or pass on a checkout link."
        );
    }
    let subscribe = format!("{} subscribe", BRANDING.cli_command());
    if !text.contains(&subscribe) {
        return text;
    }
    format!(
        "{text}\n\nLocalify Cloud Pro costs $7.99 a month or $79 a year, after a 7-day free trial that asks for a card. Do not start a checkout or pass on a checkout link: ask the user to run `{subscribe}` themselves, then try again."
    )
}

/// What an agent does when another machine refused, or did not say how a
/// control ended (docs/remote-machines.md, "MCP"). The CLI prints these as
/// JSON when its output is not a terminal.
fn remote_hint(text: &str) -> Option<String> {
    let cli = BRANDING.cli_command();
    if text.contains(r#""code": "control_disabled""#)
        || text.contains(r#""code": "background_disabled""#)
    {
        return Some(format!(
            "That machine decides who may control its services. Ask the user to allow remote controls on it: Settings → Remote access in Localify there, or `{cli} machines settings --control on` run there (with `--background-starts on` for starts while its app is closed)."
        ));
    }
    if text.contains(r#""outcome": "lost""#) {
        return Some(
            "Call service_status with the same machine to see what happened before you try again."
                .to_string(),
        );
    }
    None
}

fn prepare(tool: &Tool, args: &ToolArgs) -> Result<(Vec<String>, Option<PathBuf>)> {
    // A tool without `machine` would act on this machine instead: refuse it
    // before building or running anything (docs/remote-machines.md, "MCP").
    if args.has("machine") && !tool.params.iter().any(|param| param.name == "machine") {
        let remote: Vec<&str> = TOOLS
            .iter()
            .filter(|tool| tool.params.iter().any(|param| param.name == "machine"))
            .map(|tool| tool.name)
            .collect();
        bail!(
            "{} acts on this machine only and takes no machine. Only {} act on another machine.",
            tool.name,
            remote.join(", ")
        );
    }
    let argv = (tool.build)(args)?;
    // A tool acting on another machine has no folder of this one to read.
    let here = args.string("machine")?.is_none();
    let cwd = match args.string("cwd")? {
        Some(cwd) if tool.uses_cwd && here => Some(existing_dir(&cwd)?),
        _ => None,
    };
    Ok((argv, cwd))
}

fn existing_dir(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || !path.is_dir() {
        bail!("cwd must be the absolute path of an existing folder, got '{value}'.");
    }
    Ok(path)
}

struct CliRunner {
    executable: PathBuf,
    config_path: PathBuf,
}

impl CliRunner {
    /// Runs one CLI command and returns its output, and whether it failed.
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> (String, bool) {
        let mut command = Command::new(&self.executable);
        command
            .arg("--config")
            .arg(&self.config_path)
            .args(argv)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        match command.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let text = [stdout.trim_end(), stderr.trim_end()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let text = if text.is_empty() {
                    "Done.".to_string()
                } else {
                    text
                };
                (text, !output.status.success())
            }
            Err(error) => (
                format!("Failed to run {}: {error}", BRANDING.cli_command()),
                true,
            ),
        }
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::cloud_errors::with_next_step;
    use crate::application::ports::{CloudApiError, UpgradeHint};

    fn runner() -> CliRunner {
        CliRunner {
            executable: PathBuf::from("/bin/echo"),
            config_path: PathBuf::from("/tmp/config.toml"),
        }
    }

    fn request(method: &str, params: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string()
    }

    #[test]
    fn initialize_answers_the_requested_version_with_instructions() {
        let response = handle_line(
            &request("initialize", json!({ "protocolVersion": "2025-03-26" })),
            &runner(),
        )
        .unwrap();

        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert!(
            response["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("remove_environment")
        );
    }

    #[test]
    fn an_unknown_version_gets_the_newest() {
        let response = handle_line(
            &request("initialize", json!({ "protocolVersion": "1999-01-01" })),
            &runner(),
        )
        .unwrap();

        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_response() {
        let line = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();

        assert!(handle_line(&line, &runner()).is_none());
    }

    #[test]
    fn tools_list_has_a_schema_for_every_tool() {
        let response = handle_line(&request("tools/list", json!({})), &runner()).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();

        assert_eq!(tools.len(), TOOLS.len());
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(!tool["description"].as_str().unwrap().is_empty(), "{tool}");
        }
    }

    #[test]
    fn a_tool_call_runs_the_cli_with_the_config() {
        let response = handle_line(
            &request(
                "tools/call",
                json!({ "name": "service_stop", "arguments": { "service": "web", "env": "feat" } }),
            ),
            &runner(),
        )
        .unwrap();

        assert_eq!(response["result"]["isError"], false);
        assert_eq!(
            response["result"]["content"][0]["text"],
            "--config /tmp/config.toml stop web --env=feat"
        );
    }

    #[test]
    fn bad_arguments_are_a_tool_error() {
        let response = handle_line(
            &request(
                "tools/call",
                json!({ "name": "service_stop", "arguments": {} }),
            ),
            &runner(),
        )
        .unwrap();

        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("service")
        );
    }

    #[test]
    fn a_missing_plan_tells_the_agent_to_ask_the_user() {
        let cli = BRANDING.cli_command();
        let refused = format!("No plan. Start a 7-day trial with {cli} subscribe.");

        let text = for_an_agent(refused.clone());

        assert!(text.starts_with(&refused));
        assert!(text.contains("$7.99") && text.contains("ask the user"));
        assert_eq!(for_an_agent("Port taken.".into()), "Port taken.");
    }

    #[test]
    fn a_limit_the_user_can_raise_keeps_its_command_for_the_agent() {
        let refused = with_next_step(CloudApiError::Rejected {
            status: 403,
            code: "quota_exceeded".into(),
            message: "You already have 10 public URLs.".into(),
            upgrade: Some(UpgradeHint {
                kind: "addon_urls".into(),
                command: "localify billing".into(),
            }),
        })
        .to_string();

        let text = for_an_agent(refused.clone());

        assert!(text.starts_with(&refused));
        assert!(text.contains("localify billing"), "{text}");
        assert!(
            text.contains("the user's call") && !text.contains("$7.99"),
            "{text}"
        );
    }

    #[test]
    fn a_tool_on_another_machine_ignores_cwd() {
        let tool = TOOLS
            .iter()
            .find(|tool| tool.name == "service_logs")
            .unwrap();
        let here = ToolArgs::new(&json!({ "service": "web", "cwd": "relative/path" })).unwrap();
        let there = ToolArgs::new(
            &json!({ "service": "web", "cwd": "relative/path", "machine": "teo-mini" }),
        )
        .unwrap();

        assert!(prepare(tool, &here).is_err());
        let (argv, cwd) = prepare(tool, &there).unwrap();
        assert_eq!(cwd, None);
        assert_eq!(argv, ["logs", "web", "--machine=teo-mini"]);
    }

    #[test]
    fn a_tool_without_machine_refuses_one_instead_of_acting_here() {
        let local_only: Vec<&Tool> = TOOLS
            .iter()
            .filter(|tool| !tool.params.iter().any(|param| param.name == "machine"))
            .collect();
        assert!(local_only.len() > 30, "{}", local_only.len());

        for tool in local_only {
            let response = handle_line(
                &request(
                    "tools/call",
                    json!({
                        "name": tool.name,
                        "arguments": { "machine": "B", "cwd": "/", "command": ["true"] },
                    }),
                ),
                &runner(),
            )
            .unwrap();

            let text = response["result"]["content"][0]["text"].as_str().unwrap();
            assert_eq!(response["result"]["isError"], true, "{}: {text}", tool.name);
            assert!(
                text.starts_with(&format!("{} acts on this machine only", tool.name))
                    && text.contains("service_status"),
                "{}: {text}",
                tool.name
            );
        }
    }

    #[test]
    fn an_error_from_another_machine_reaches_the_agent_with_its_code_and_what_to_do() {
        let refused = "{\n  \"outcome\": \"refused\",\n  \"code\": \"control_disabled\",\n  \"message\": \"teo-mini doesn't allow remote controls.\"\n}";

        let text = for_an_agent(refused.to_string());

        assert!(text.starts_with(refused));
        assert!(
            text.contains("Ask the user to allow remote controls"),
            "{text}"
        );

        let lost = "{\n  \"outcome\": \"lost\",\n  \"code\": \"timeout\"\n}";
        assert!(
            for_an_agent(lost.to_string()).contains("Call service_status with the same machine")
        );
    }

    #[test]
    fn an_unknown_method_is_an_error() {
        let response = handle_line(&request("resources/list", json!({})), &runner()).unwrap();

        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }
}
