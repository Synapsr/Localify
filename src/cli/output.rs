//! Where a command's output goes: text for a person on a terminal, JSON for
//! scripts and agents.

use std::io::{self, IsTerminal};

use anyhow::Result;
use serde::Serialize;

/// JSON when asked with `--json`, or when stdout is not a terminal, such as a
/// pipe or an agent's MCP call.
pub fn wants_json(json_flag: bool) -> bool {
    json_flag || !io::stdout().is_terminal()
}

/// A person can answer a question: never prompt otherwise.
pub fn can_prompt() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

pub fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// A line beside the result: on stdout with text, on stderr with JSON, so
/// stdout stays one JSON document.
pub fn note(json: bool, line: &str) {
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}
