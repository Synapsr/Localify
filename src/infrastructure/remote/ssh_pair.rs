//! Pairing over SSH (`docs/remote-machines.md`, "Pairing over SSH"; ADR
//! 0035): the JSON lines between `localify machines pair --over-ssh` on the
//! initiator and `localify machines pair --accept --for-ssh` on the machine
//! it pairs with, and the `ssh` command line the initiator runs.
//!
//! The helper's stdout carries only these lines, each under 4 KiB with
//! `"v": 1`; its stderr is diagnostics, never parsed. The initiator writes
//! `confirm` or `cancel` to its stdin.

use std::io::{self, BufRead, Read};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::pairing::{Outcome, PinState};

/// The protocol version both lines carry.
pub const VERSION: u64 = 1;
/// A line, its newline included, is at most this long.
pub const MAX_LINE: usize = 4096;

/// What the helper's stdout says, in this order: `ready`, then `code`
/// unless the attempt ended first, then `result`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum HelperLine {
    /// The helper's daemon holds a one-shot acceptor for this account and
    /// this initiating machine.
    Ready {
        machine_id: String,
        account: String,
        attempt: String,
    },
    Code {
        attempt: String,
        code: String,
    },
    Result {
        attempt: String,
        outcome: Outcome,
        local: PinState,
        peer: PinState,
        detail: String,
    },
}

/// What the initiator writes to the helper's stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlLine {
    /// Confirm, only if this is the helper's own code.
    Confirm(String),
    Cancel,
}

/// Why a line ends the attempt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("a line is longer than 4 KiB")]
    TooLong,
    #[error("a line is not one this protocol knows: {0}")]
    Malformed(String),
    #[error("a line says version {0}, and only version 1 is known")]
    Version(String),
    #[error("the {0} line came out of order or twice")]
    OutOfOrder(&'static str),
    #[error("a line names another attempt")]
    Foreign,
}

impl HelperLine {
    /// The line to write, newline included.
    pub fn to_line(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(fields) = &mut value {
            fields.insert("v".into(), VERSION.into());
        }
        format!("{value}\n")
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::Code { .. } => "code",
            Self::Result { .. } => "result",
        }
    }

    /// Bounds on what a line may carry, beyond its shape.
    fn check(&self) -> Result<(), ProtocolError> {
        let malformed = |what: &str| Err(ProtocolError::Malformed(what.to_string()));
        match self {
            Self::Ready {
                machine_id,
                account,
                attempt,
            } => {
                if !is_id(machine_id) || !is_id(account) || !is_id(attempt) {
                    return malformed("ready carries an empty or invalid id");
                }
            }
            Self::Code { attempt, code } => {
                if !is_id(attempt) || !is_code(code) {
                    return malformed("code is not six digits in two groups");
                }
            }
            Self::Result { attempt, .. } => {
                if !is_id(attempt) {
                    return malformed("result carries an invalid attempt");
                }
            }
        }
        Ok(())
    }
}

impl ControlLine {
    /// The line to write, newline included.
    pub fn to_line(&self) -> String {
        let value = match self {
            Self::Confirm(code) => serde_json::json!({ "v": VERSION, "confirm": code }),
            Self::Cancel => serde_json::json!({ "v": VERSION, "cancel": true }),
        };
        format!("{value}\n")
    }
}

/// An id as the contract's machine ids, user ids and attempts are: short,
/// printable, no spaces.
fn is_id(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@'))
}

/// `482 913`.
fn is_code(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7
        && bytes[3] == b' '
        && bytes[..3].iter().chain(&bytes[4..]).all(u8::is_ascii_digit)
}

/// The object of a line, once its version is checked.
fn versioned(line: &[u8]) -> Result<Map<String, Value>, ProtocolError> {
    if line.len() > MAX_LINE {
        return Err(ProtocolError::TooLong);
    }
    let value: Value = serde_json::from_slice(line)
        .map_err(|error| ProtocolError::Malformed(error.to_string()))?;
    let Value::Object(mut fields) = value else {
        return Err(ProtocolError::Malformed("not a JSON object".into()));
    };
    match fields.remove("v") {
        Some(Value::Number(v)) if v.as_u64() == Some(VERSION) => Ok(fields),
        Some(other) => Err(ProtocolError::Version(other.to_string())),
        None => Err(ProtocolError::Version("nothing".into())),
    }
}

/// One line of the helper's stdout, on its own.
pub fn parse_helper_line(line: &[u8]) -> Result<HelperLine, ProtocolError> {
    let fields = versioned(line)?;
    let parsed: HelperLine = serde_json::from_value(Value::Object(fields))
        .map_err(|error| ProtocolError::Malformed(error.to_string()))?;
    parsed.check()?;
    Ok(parsed)
}

/// One line of the helper's stdin: exactly `confirm` with a code, or
/// `cancel: true`, and nothing else.
pub fn parse_control_line(line: &[u8]) -> Result<ControlLine, ProtocolError> {
    let fields = versioned(line)?;
    let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    keys.sort_unstable();
    match (keys.as_slice(), fields.get("confirm"), fields.get("cancel")) {
        (["confirm"], Some(Value::String(code)), _) => Ok(ControlLine::Confirm(code.clone())),
        (["cancel"], _, Some(Value::Bool(true))) => Ok(ControlLine::Cancel),
        _ => Err(ProtocolError::Malformed(
            "expected exactly confirm with a code, or cancel: true".into(),
        )),
    }
}

/// The helper's lines as the initiator takes them: in order, each once,
/// all of one attempt.
#[derive(Debug, Default)]
pub struct HelperLines {
    attempt: Option<String>,
    code: bool,
    result: bool,
}

impl HelperLines {
    /// Takes the next line, or says why it ends the attempt.
    pub fn take(&mut self, line: &[u8]) -> Result<HelperLine, ProtocolError> {
        let parsed = parse_helper_line(line)?;
        let in_order = match &parsed {
            HelperLine::Ready { .. } => self.attempt.is_none(),
            HelperLine::Code { .. } => self.attempt.is_some() && !self.code && !self.result,
            HelperLine::Result { .. } => self.attempt.is_some() && !self.result,
        };
        if !in_order {
            return Err(ProtocolError::OutOfOrder(parsed.name()));
        }
        match &parsed {
            HelperLine::Ready { attempt, .. } => self.attempt = Some(attempt.clone()),
            HelperLine::Code { attempt, .. } | HelperLine::Result { attempt, .. } => {
                if self.attempt.as_ref() != Some(attempt) {
                    return Err(ProtocolError::Foreign);
                }
            }
        }
        self.code |= matches!(parsed, HelperLine::Code { .. });
        self.result |= matches!(parsed, HelperLine::Result { .. });
        Ok(parsed)
    }
}

/// Reads one line of at most [`MAX_LINE`] bytes, without its newline.
/// `None` at the end of the stream; a longer line is
/// [`ProtocolError::TooLong`], and what follows it is not read.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Result<Vec<u8>, ProtocolError>>> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    } else if read == MAX_LINE {
        return Ok(Some(Err(ProtocolError::TooLong)));
    }
    Ok(Some(Ok(line)))
}

/// Why an argument of the `ssh` command is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgumentError {
    #[error("The SSH destination must be one host or user@host that does not start with '-'.")]
    Destination,
    #[error("--remote-localify must be one absolute path, without a newline or a NUL.")]
    RemoteLocalify,
    #[error("A machine id or account id holds a newline or a NUL.")]
    Unquotable,
    #[error(
        "Pairing over SSH needs OpenSSH 8.7 or newer on this machine, for its StdinNull and ForkAfterAuthentication options. `ssh -V` says: {0}"
    )]
    OldSsh(String),
}

/// The oldest OpenSSH that has every option of [`SSH_OPTIONS`].
const MIN_OPENSSH: (u32, u32) = (8, 7);

/// Checks `ssh -V`'s output before `ssh` runs with options it may not
/// know. Another ssh, or a version that does not read as `OpenSSH_X.Y`,
/// fails closed.
pub fn require_openssh(version_output: &str) -> Result<(), ArgumentError> {
    match openssh_version(version_output) {
        Some(version) if version >= MIN_OPENSSH => Ok(()),
        _ => {
            let said = version_output.lines().next().unwrap_or_default().trim();
            let said = if said.is_empty() { "nothing" } else { said };
            Err(ArgumentError::OldSsh(said.to_string()))
        }
    }
}

/// `(8, 7)` from `OpenSSH_8.7p1, LibreSSL 3.3.6`.
fn openssh_version(output: &str) -> Option<(u32, u32)> {
    let version = output
        .split_whitespace()
        .find_map(|word| word.strip_prefix("OpenSSH_"))?;
    let (major, rest) = version.split_once('.')?;
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The options the contract fixes: no shell, no prompt, a host key that
/// must already be trusted, stdin kept open, nothing forwarded, no shared
/// connection, 15 s to connect.
pub const SSH_OPTIONS: [&str; 22] = [
    "-T",
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=yes",
    "-o",
    "StdinNull=no",
    "-o",
    "ForkAfterAuthentication=no",
    "-o",
    "ForwardAgent=no",
    "-o",
    "ForwardX11=no",
    "-o",
    "ClearAllForwardings=yes",
    "-o",
    "ControlMaster=no",
    "-o",
    "ControlPath=none",
    "-o",
    "ConnectTimeout=15",
    "--",
];

/// `ssh`'s arguments, run without a local shell: the fixed options, the
/// destination as one operand, and the remote command as one string.
pub fn ssh_args(destination: &str, remote_command: &str) -> Result<Vec<String>, ArgumentError> {
    let refused = destination.is_empty()
        || destination.starts_with('-')
        || destination
            .chars()
            .any(|c| c.is_control() || c.is_whitespace());
    if refused {
        return Err(ArgumentError::Destination);
    }
    let mut args: Vec<String> = SSH_OPTIONS
        .iter()
        .map(|option| option.to_string())
        .collect();
    args.push(destination.to_string());
    args.push(remote_command.to_string());
    Ok(args)
}

/// The command the remote shell runs: the executable and fixed
/// arguments, each quoted for a POSIX shell.
pub fn remote_command(
    executable: &str,
    from: &str,
    account: &str,
) -> Result<String, ArgumentError> {
    let words = [
        executable,
        "machines",
        "pair",
        "--accept",
        "--for-ssh",
        "--from",
        from,
        "--account",
        account,
    ];
    if words.iter().any(|word| word.contains(['\n', '\0'])) {
        return Err(ArgumentError::Unquotable);
    }
    Ok(words.map(shell_quote).join(" "))
}

/// Checks `--remote-localify`: one absolute path.
pub fn remote_executable(path: &str) -> Result<&str, ArgumentError> {
    if !path.starts_with('/') || path.contains(['\n', '\0']) {
        return Err(ArgumentError::RemoteLocalify);
    }
    Ok(path)
}

/// Single quotes, with each quote inside closed, escaped and reopened.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> String {
        HelperLine::Ready {
            machine_id: "mB".into(),
            account: "u1".into(),
            attempt: "at1".into(),
        }
        .to_line()
    }

    fn code(attempt: &str) -> String {
        HelperLine::Code {
            attempt: attempt.into(),
            code: "482 913".into(),
        }
        .to_line()
    }

    fn result(attempt: &str) -> String {
        HelperLine::Result {
            attempt: attempt.into(),
            outcome: Outcome::Paired,
            local: PinState::Pinned,
            peer: PinState::Pinned,
            detail: String::new(),
        }
        .to_line()
    }

    fn take(lines: &mut HelperLines, line: &str) -> Result<HelperLine, ProtocolError> {
        lines.take(line.trim_end().as_bytes())
    }

    #[test]
    fn lines_round_trip_with_their_version() {
        let line = ready();
        assert!(line.ends_with('\n'));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["event"], "ready");
        assert_eq!(value["machineId"], "mB");
        let mut lines = HelperLines::default();
        assert!(matches!(
            take(&mut lines, &line),
            Ok(HelperLine::Ready { .. })
        ));
        assert!(matches!(
            take(&mut lines, &code("at1")),
            Ok(HelperLine::Code { .. })
        ));
        let Ok(HelperLine::Result { outcome, local, .. }) = take(&mut lines, &result("at1")) else {
            panic!("no result");
        };
        assert_eq!((outcome, local), (Outcome::Paired, PinState::Pinned));
    }

    #[test]
    fn a_cancel_before_any_code_is_ready_then_result() {
        let mut lines = HelperLines::default();
        take(&mut lines, &ready()).unwrap();
        take(&mut lines, &result("at1")).unwrap();
    }

    #[test]
    fn malformed_lines_end_the_attempt() {
        for line in [
            "not json",
            "[1, 2]",
            r#"{"v": 1}"#,
            r#"{"v": 1, "event": "hello"}"#,
            r#"{"v": 1, "event": "ready", "machineId": "mB"}"#,
            r#"{"v": 1, "event": "ready", "machineId": "", "account": "u1", "attempt": "a"}"#,
            r#"{"v": 1, "event": "ready", "machineId": "m B", "account": "u1", "attempt": "a"}"#,
            r#"{"v": 1, "event": "code", "attempt": "a", "code": "482913"}"#,
            r#"{"v": 1, "event": "code", "attempt": "a", "code": "48a 913"}"#,
            r#"{"v": 1, "event": "result", "attempt": "a", "outcome": "won", "local": "pinned", "peer": "pinned", "detail": ""}"#,
        ] {
            assert!(
                matches!(
                    parse_helper_line(line.as_bytes()),
                    Err(ProtocolError::Malformed(_))
                ),
                "{line}"
            );
        }
    }

    #[test]
    fn a_version_other_than_1_is_refused() {
        for line in [
            r#"{"event": "ready", "machineId": "mB", "account": "u1", "attempt": "a"}"#,
            r#"{"v": 2, "event": "ready", "machineId": "mB", "account": "u1", "attempt": "a"}"#,
            r#"{"v": "1", "event": "ready", "machineId": "mB", "account": "u1", "attempt": "a"}"#,
            r#"{"v": 1.5, "cancel": true}"#,
        ] {
            assert!(
                matches!(
                    parse_helper_line(line.as_bytes()),
                    Err(ProtocolError::Version(_))
                ),
                "{line}"
            );
        }
        assert!(matches!(
            parse_control_line(br#"{"v": 2, "cancel": true}"#),
            Err(ProtocolError::Version(_))
        ));
    }

    #[test]
    fn duplicate_or_out_of_order_lines_end_the_attempt() {
        let mut lines = HelperLines::default();
        assert_eq!(
            take(&mut lines, &code("at1")),
            Err(ProtocolError::OutOfOrder("code"))
        );
        let mut lines = HelperLines::default();
        assert_eq!(
            take(&mut lines, &result("at1")),
            Err(ProtocolError::OutOfOrder("result"))
        );
        let mut lines = HelperLines::default();
        take(&mut lines, &ready()).unwrap();
        assert_eq!(
            take(&mut lines, &ready()),
            Err(ProtocolError::OutOfOrder("ready"))
        );
        take(&mut lines, &code("at1")).unwrap();
        assert_eq!(
            take(&mut lines, &code("at1")),
            Err(ProtocolError::OutOfOrder("code"))
        );
        take(&mut lines, &result("at1")).unwrap();
        assert_eq!(
            take(&mut lines, &result("at1")),
            Err(ProtocolError::OutOfOrder("result"))
        );
        // A code after the result.
        let mut lines = HelperLines::default();
        take(&mut lines, &ready()).unwrap();
        take(&mut lines, &result("at1")).unwrap();
        assert_eq!(
            take(&mut lines, &code("at1")),
            Err(ProtocolError::OutOfOrder("code"))
        );
    }

    #[test]
    fn a_line_of_another_attempt_is_foreign() {
        let mut lines = HelperLines::default();
        take(&mut lines, &ready()).unwrap();
        assert_eq!(take(&mut lines, &code("at2")), Err(ProtocolError::Foreign));
        let mut lines = HelperLines::default();
        take(&mut lines, &ready()).unwrap();
        assert_eq!(
            take(&mut lines, &result("other")),
            Err(ProtocolError::Foreign)
        );
    }

    #[test]
    fn lines_are_capped_at_4_kib() {
        let long = format!(
            r#"{{"v": 1, "event": "result", "attempt": "a", "outcome": "cancelled", "local": "not_pinned", "peer": "not_pinned", "detail": "{}"}}"#,
            "x".repeat(MAX_LINE)
        );
        assert_eq!(
            parse_helper_line(long.as_bytes()),
            Err(ProtocolError::TooLong)
        );

        let stream = format!("{}\n{}", "y".repeat(MAX_LINE + 10), ready());
        let mut reader = io::Cursor::new(stream.into_bytes());
        assert_eq!(
            read_line(&mut reader).unwrap(),
            Some(Err(ProtocolError::TooLong))
        );

        let fits = format!("{}\nrest", "z".repeat(MAX_LINE - 1));
        let mut reader = io::Cursor::new(fits.into_bytes());
        let Some(Ok(line)) = read_line(&mut reader).unwrap() else {
            panic!("a line of 4095 bytes and its newline fits");
        };
        assert_eq!(line.len(), MAX_LINE - 1);
        assert_eq!(read_line(&mut reader).unwrap(), Some(Ok(b"rest".to_vec())));
        assert_eq!(read_line(&mut reader).unwrap(), None);
    }

    #[test]
    fn control_lines_are_exactly_confirm_or_cancel() {
        assert_eq!(
            parse_control_line(
                ControlLine::Confirm("482 913".into())
                    .to_line()
                    .trim_end()
                    .as_bytes()
            ),
            Ok(ControlLine::Confirm("482 913".into()))
        );
        assert_eq!(
            parse_control_line(ControlLine::Cancel.to_line().trim_end().as_bytes()),
            Ok(ControlLine::Cancel)
        );
        for line in [
            r#"{"v": 1}"#,
            r#"{"v": 1, "cancel": false}"#,
            r#"{"v": 1, "confirm": 482913}"#,
            r#"{"v": 1, "confirm": "482 913", "cancel": true}"#,
            r#"{"v": 1, "confirm": "482 913", "also": 1}"#,
            "yes",
        ] {
            assert!(
                matches!(
                    parse_control_line(line.as_bytes()),
                    Err(ProtocolError::Malformed(_))
                ),
                "{line}"
            );
        }
    }

    #[test]
    fn ssh_runs_with_exactly_the_contracts_options() {
        let args = ssh_args("teo@mini.local", "'localify' 'machines'").unwrap();
        assert_eq!(
            args.join(" "),
            "-T -o BatchMode=yes -o StrictHostKeyChecking=yes -o StdinNull=no \
             -o ForkAfterAuthentication=no -o ForwardAgent=no -o ForwardX11=no \
             -o ClearAllForwardings=yes -o ControlMaster=no -o ControlPath=none \
             -o ConnectTimeout=15 -- teo@mini.local 'localify' 'machines'"
        );
        // The destination and the command are one operand each.
        assert_eq!(args[args.len() - 2], "teo@mini.local");
        assert_eq!(args[args.len() - 1], "'localify' 'machines'");
    }

    #[test]
    fn a_destination_that_could_be_an_option_is_refused() {
        for destination in [
            "",
            "-oProxyCommand=touch /tmp/x",
            "-p",
            "host name",
            "host\nname",
            "host\0",
        ] {
            assert_eq!(
                ssh_args(destination, "x"),
                Err(ArgumentError::Destination),
                "{destination:?}"
            );
        }
        assert!(ssh_args("mini", "x").is_ok());
        assert!(ssh_args("teo@[fe80::1%en0]", "x").is_ok());
    }

    #[test]
    fn the_remote_command_quotes_every_word() {
        assert_eq!(
            remote_command("localify", "mA", "u1").unwrap(),
            "'localify' 'machines' 'pair' '--accept' '--for-ssh' '--from' 'mA' '--account' 'u1'"
        );
        let command = remote_command("/Users/t o/it's $HOME/`x`/localify", "mA", "u1").unwrap();
        assert!(
            command.starts_with(r"'/Users/t o/it'\''s $HOME/`x`/localify' "),
            "{command}"
        );
        // What a POSIX shell makes of it.
        let output = std::process::Command::new("sh")
            .args(["-c", &format!("printf '%s\\n' {command}")])
            .output()
            .unwrap();
        let words: Vec<&str> = std::str::from_utf8(&output.stdout)
            .unwrap()
            .lines()
            .collect();
        assert_eq!(words[0], "/Users/t o/it's $HOME/`x`/localify");
        assert_eq!(words.len(), 9);
        assert_eq!(&words[5..], ["--from", "mA", "--account", "u1"]);
    }

    #[test]
    fn a_remote_path_must_be_one_absolute_path() {
        assert_eq!(
            remote_executable("/opt/localify/bin/localify"),
            Ok("/opt/localify/bin/localify")
        );
        for path in ["localify", "~/bin/localify", "/bin/x\nrm", "/bin/x\0", ""] {
            assert_eq!(
                remote_executable(path),
                Err(ArgumentError::RemoteLocalify),
                "{path:?}"
            );
        }
        assert_eq!(
            remote_command("/bin/localify", "m\nA", "u1"),
            Err(ArgumentError::Unquotable)
        );
        assert_eq!(
            remote_command("/bin/localify", "mA", "u\u{0}1"),
            Err(ArgumentError::Unquotable)
        );
    }

    #[test]
    fn only_openssh_8_7_or_newer_runs() {
        for ok in [
            "OpenSSH_8.7p1, OpenSSL 3.0.2 15 Mar 2022",
            "OpenSSH_8.9p1 Ubuntu-3ubuntu0.10, OpenSSL 3.0.2",
            "OpenSSH_9.0",
            "OpenSSH_10.3p1, LibreSSL 3.3.6",
        ] {
            assert_eq!(require_openssh(ok), Ok(()), "{ok}");
        }
        for old in [
            "OpenSSH_8.6p1, OpenSSL 1.1.1k",
            "OpenSSH_7.4p1, OpenSSL 1.0.2k-fips",
            "OpenSSH_for_Windows_8.1p1, LibreSSL 3.0.2",
            "Sun_SSH_1.1",
            "OpenSSH_",
            "",
        ] {
            let error = require_openssh(old).unwrap_err();
            assert!(error.to_string().contains("OpenSSH 8.7 or newer"), "{old}");
        }
        let error = require_openssh("OpenSSH_8.6p1, OpenSSL 1.1.1k\n").unwrap_err();
        assert!(
            error
                .to_string()
                .ends_with("says: OpenSSH_8.6p1, OpenSSL 1.1.1k")
        );
    }
}
