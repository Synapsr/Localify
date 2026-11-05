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
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": failed,
    }))
}

fn prepare(tool: &Tool, args: &ToolArgs) -> Result<(Vec<String>, Option<PathBuf>)> {
    let argv = (tool.build)(args)?;
    let cwd = match args.string("cwd")? {
        Some(cwd) if tool.uses_cwd => Some(existing_dir(&cwd)?),
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
    fn an_unknown_method_is_an_error() {
        let response = handle_line(&request("resources/list", json!({})), &runner()).unwrap();

        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }
}
