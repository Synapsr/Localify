//! `logs <service> --machine`: a run's output on another machine, as plain
//! lines (docs/remote-machines.md, "Output"). Reads are bounded on the
//! target; a cursor `<runId>:<offset>` reads on from where the last call
//! stopped, as it does locally.

use std::collections::VecDeque;
use std::io::Write;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use super::inventory::{Item, ServiceQuery, fetch, find, process_state};
use super::{Console, Peer, Refusal, remote_error};
use crate::infrastructure::mgmt_client::{MgmtSocketClient, MgmtStream};
use crate::infrastructure::remote::wire::{MAX_READ_BYTES, OutputData, OutputLine};
use crate::infrastructure::runs::RunId;

/// Without a cursor, the last lines come from at most this much of the end.
const TAIL_BYTES: u64 = 64 * 1024;
/// How much one `--cursor` call reads at most; the next cursor goes on.
const READ_BUDGET: u64 = 256 * 1024;

pub struct LogsQuery<'a> {
    pub service: ServiceQuery<'a>,
    pub lines: usize,
    pub follow: bool,
    pub cursor: Option<&'a str>,
}

/// Prints the last lines of the service's run, or what came after the
/// cursor, then follows it when asked. A reader that is not a terminal also
/// gets the cursor to pass next time.
pub fn logs<O: Write, E: Write>(
    client: &MgmtSocketClient,
    peer: &Peer,
    query: &LogsQuery<'_>,
    console: &mut Console<O, E>,
) -> Result<i32> {
    let cursor = query.cursor.map(Cursor::parse).transpose()?;
    let (snapshot, _) = fetch(client, peer)?;
    let items = snapshot.shared_items(peer)?;
    let (item, _) = find(&items, &query.service, &peer.name)?;
    let run_id = readable_run(item, &peer.name)?;
    writeln!(
        console.err,
        "'{}' on {}: {}.\n",
        item.service.name,
        peer.name,
        process_state(item)
    )?;
    let mut reader = Reader {
        client,
        peer,
        query: &query.service,
        run_id,
        service: item.service.name.clone(),
        console,
    };
    let first = match &cursor {
        Some(cursor) if cursor.run_id == reader.run_id => reader.read_on(cursor.offset),
        Some(_) => {
            reader.note(&format!(
                "'{}' restarted since that cursor: reading its current run from the start.",
                reader.service
            ))?;
            reader.read_on(0)
        }
        None => reader.tail(query.lines),
    };
    // The service restarted between the snapshot and the read: its new run
    // is read as a cursor of another run would be.
    let mut offset = match first {
        Err(error) if is_gone(&error) => {
            reader.move_to_current_run()?;
            match cursor {
                Some(_) => reader.read_on(0)?,
                None => reader.tail(query.lines)?,
            }
        }
        other => other?,
    };
    if query.follow {
        offset = reader.follow(offset)?;
    }
    if query.cursor.is_some() || reader.console.json {
        writeln!(
            reader.console.err,
            "\nNext cursor: {}:{offset}",
            reader.run_id
        )?;
    }
    Ok(0)
}

/// `gone`: the run is no longer the service's last run.
fn is_gone(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<Refusal>()
        .is_some_and(|refusal| refusal.code == "gone")
}

/// Where a read goes on from: `<runId>:<offset>`.
#[derive(Debug, PartialEq, Eq)]
struct Cursor {
    run_id: String,
    offset: u64,
}

impl Cursor {
    fn parse(value: &str) -> Result<Self> {
        let invalid = || {
            format!(
                "'{value}' is not a cursor of another machine. Pass back the `<runId>:<offset>` the previous `logs --machine` call printed."
            )
        };
        let (run_id, offset) = value.split_once(':').with_context(invalid)?;
        RunId::parse(run_id).with_context(invalid)?;
        Ok(Self {
            run_id: run_id.to_string(),
            offset: offset.parse().with_context(invalid)?,
        })
    }
}

/// The run whose output can be read, or why none can.
fn readable_run(item: &Item, machine: &str) -> Result<String> {
    if let (true, Some(run_id)) = (item.output.available, &item.output.run_id) {
        return Ok(run_id.clone());
    }
    let service = &item.service.name;
    let message = match item.output.reason.as_deref() {
        Some("terminal") => {
            format!("'{service}' runs in a terminal on {machine}: its output is only there.")
        }
        Some("unknown_owner") => format!(
            "An unknown process holds the port of '{service}' on {machine}, and its output can't be read."
        ),
        Some("legacy_run") => format!(
            "'{service}' started before this version of Localify on {machine}. Relaunch it there to share its output."
        ),
        _ => format!("'{service}' has not run on {machine} yet."),
    };
    Err(anyhow!(Refusal::new("output_unavailable", message)))
}

struct Reader<'a, O, E> {
    client: &'a MgmtSocketClient,
    peer: &'a Peer,
    query: &'a ServiceQuery<'a>,
    /// The run being read; it moves to the next run when the service restarts.
    run_id: String,
    service: String,
    console: &'a mut Console<O, E>,
}

impl<O: Write, E: Write> Reader<'_, O, E> {
    fn note(&mut self, line: &str) -> Result<()> {
        writeln!(self.console.err, "{line}")?;
        Ok(())
    }

    fn read(&self, offset: u64) -> Result<OutputData> {
        let value = self
            .client
            .remote_output_read(&self.peer.id, &self.run_id, offset, MAX_READ_BYTES)
            .map_err(|error| self.restarted(remote_error(error, self.peer)))?;
        data(value)
    }

    /// `gone` from a read means the service restarted since it was looked at.
    fn restarted(&self, error: anyhow::Error) -> anyhow::Error {
        match error.downcast_ref::<Refusal>() {
            Some(refusal) if refusal.code == "gone" => anyhow!(Refusal::new(
                "gone",
                format!(
                    "'{}' restarted on {} again. Read its output again.",
                    self.service, self.peer.name
                ),
            )),
            _ => error,
        }
    }

    /// After `gone`: moves to the service's current run, as the target's
    /// snapshot shows it, to read it from offset 0.
    fn move_to_current_run(&mut self) -> Result<()> {
        let (snapshot, _) = fetch(self.client, self.peer)?;
        let items = snapshot.shared_items(self.peer)?;
        let (item, _) = find(&items, self.query, &self.peer.name)?;
        let run_id = readable_run(item, &self.peer.name)?;
        if run_id == self.run_id {
            return Err(anyhow!(Refusal::new(
                "gone",
                format!(
                    "'{}' restarted on {}, and its new run isn't shown yet. Read its output again.",
                    self.service, self.peer.name
                ),
            )));
        }
        self.note(&format!(
            "'{}' restarted on {}: reading its new run from the start.",
            self.service, self.peer.name
        ))?;
        self.run_id = run_id;
        Ok(())
    }

    /// Prints from `offset` until caught up, the run ended, or
    /// [`READ_BUDGET`] was read; returns where it stopped.
    fn read_on(&mut self, mut offset: u64) -> Result<u64> {
        let started = offset;
        loop {
            let chunk = self.read(offset)?;
            self.print(&chunk)?;
            let caught_up = chunk.next == offset;
            offset = chunk.next;
            if caught_up || chunk.ended || offset - started >= READ_BUDGET {
                return Ok(offset);
            }
        }
    }

    /// The last `count` lines: a follow from the last [`TAIL_BYTES`] gives
    /// where they start, and reads go on to the end.
    fn tail(&mut self, count: usize) -> Result<u64> {
        let mut follow = self.open_follow(json!({ "tail": TAIL_BYTES }))?;
        let first = match self.next_data(&mut follow)? {
            Followed::Data(data) => data,
            Followed::Ended(refusal) => return Err(anyhow!(refusal)),
            Followed::Closed => return Ok(0),
        };
        drop(follow);
        let start = first.offset;
        let mut lines = VecDeque::new();
        let mut chunk = first;
        loop {
            lines.extend(chunk.lines.iter().cloned());
            while lines.len() > count {
                lines.pop_front();
            }
            if chunk.ended || chunk.next - start >= READ_BUDGET {
                break;
            }
            let next = self.read(chunk.next)?;
            if next.next == chunk.next && !next.ended {
                break;
            }
            chunk = next;
        }
        self.print_lines(lines.iter())?;
        self.print_end(&chunk)?;
        Ok(chunk.next)
    }

    /// Prints new lines as they come, until the run ends. When the service
    /// restarts, follows its new run from offset 0.
    fn follow(&mut self, mut offset: u64) -> Result<u64> {
        let mut follow = self.open_follow(json!({ "offset": offset }))?;
        loop {
            match self.next_data(&mut follow)? {
                Followed::Data(chunk) => {
                    self.print(&chunk)?;
                    offset = chunk.next;
                    if chunk.ended {
                        self.note(&format!("The run of '{}' ended.", self.service))?;
                        return Ok(offset);
                    }
                }
                Followed::Ended(refusal) if refusal.code == "gone" => {
                    drop(follow);
                    self.move_to_current_run()?;
                    offset = 0;
                    follow = self.open_follow(json!({ "offset": offset }))?;
                }
                Followed::Ended(refusal) => return Err(anyhow!(refusal)),
                Followed::Closed => return Ok(offset),
            }
        }
    }

    fn open_follow(&self, from: Value) -> Result<MgmtStream> {
        let mut command = json!({
            "cmd": "remote_output_follow",
            "machineId": self.peer.id,
            "runId": self.run_id,
        });
        if let (Some(command), Some(from)) = (command.as_object_mut(), from.as_object()) {
            command.extend(from.clone());
        }
        self.client
            .remote_stream(&command)
            .map_err(|error| remote_error(error, self.peer))
    }

    fn next_data(&self, follow: &mut MgmtStream) -> Result<Followed> {
        let Some(event) = follow
            .next_answer()
            .map_err(|error| remote_error(error, self.peer))?
        else {
            return Ok(Followed::Closed);
        };
        match event["event"].as_str() {
            Some("data") => Ok(Followed::Data(data(event)?)),
            Some("ended") => Ok(Followed::Ended(Refusal::from_target(
                event["code"].as_str().unwrap_or_default(),
                event["message"].as_str().unwrap_or_default(),
                &self.peer.name,
            ))),
            _ => Ok(Followed::Closed),
        }
    }

    fn print(&mut self, chunk: &OutputData) -> Result<()> {
        if chunk.rotated.is_some() {
            self.note("Earlier output was dropped.")?;
        }
        self.print_lines(chunk.lines.iter())?;
        self.print_end(chunk)
    }

    fn print_lines<'l>(&mut self, lines: impl Iterator<Item = &'l OutputLine>) -> Result<()> {
        for line in lines {
            if line.cut {
                write!(self.console.out, "{}", line.text)?;
            } else {
                writeln!(self.console.out, "{}", line.text)?;
            }
        }
        Ok(())
    }

    fn print_end(&mut self, chunk: &OutputData) -> Result<()> {
        if chunk.ended && chunk.interrupted {
            let line = format!(
                "Output was cut off when Localify stopped on {}.",
                self.peer.name
            );
            self.note(&line)?;
        }
        Ok(())
    }
}

enum Followed {
    Data(OutputData),
    /// The follow stopped before its run did, with the target's reason.
    Ended(Refusal),
    /// The daemon closed the stream.
    Closed,
}

fn data(mut value: Value) -> Result<OutputData> {
    // A follow event carries `event` beside the data's own fields.
    if let Some(fields) = value.as_object_mut() {
        fields.remove("event");
    }
    serde_json::from_value(value).context("The daemon sent output this version cannot read")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::remote::inventory::tests::item;

    #[test]
    fn a_cursor_is_a_run_id_and_an_offset() {
        assert_eq!(
            Cursor::parse("RRRRRRRRRRRRRRRRRRRRRR:120").unwrap(),
            Cursor {
                run_id: "RRRRRRRRRRRRRRRRRRRRRR".into(),
                offset: 120
            }
        );
        assert!(Cursor::parse("120").is_err());
        assert!(Cursor::parse("short:1").is_err());
        assert!(Cursor::parse("RRRRRRRRRRRRRRRRRRRRRR:-1").is_err());
    }

    #[test]
    fn output_that_cannot_be_read_says_why() {
        let mut value = item("app", None, "web");
        value["output"] = json!({ "available": false, "reason": "terminal" });
        let terminal: Item = serde_json::from_value(value).unwrap();

        let error = readable_run(&terminal, "teo-mini").unwrap_err();

        let refusal = error.downcast_ref::<Refusal>().unwrap();
        assert_eq!(refusal.code, "output_unavailable");
        assert_eq!(
            refusal.message,
            "'web' runs in a terminal on teo-mini: its output is only there."
        );
    }

    #[test]
    fn a_follow_event_reads_as_output() {
        let event = json!({
            "event": "data", "re": "f1", "runId": "R", "offset": 0, "next": 6,
            "lines": [{ "text": "ready" }], "ended": false,
        });

        let data = data(event).unwrap();

        assert_eq!(data.next, 6);
        assert_eq!(data.lines[0].text, "ready");
    }
}
