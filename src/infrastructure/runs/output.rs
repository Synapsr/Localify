//! A run's output stream, its finalization lock and its `end.json`.
//!
//! The stream is made of 4 MiB segments named by their index (`0.log`,
//! `1.log`, …): segment `k` holds logical bytes `[k × 4 MiB, (k + 1) × 4 MiB)`.
//! The writer keeps the two newest, so between 4 and 8 MiB of the end of a run
//! stay readable. Offsets are logical bytes from the start of the run and are
//! never reused. Reads are bounded: they never read a segment to its end.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const SEGMENT_SIZE: u64 = 4 * 1024 * 1024;
/// A longer line arrives in pieces of at most this many bytes.
pub const MAX_LINE: usize = 8 * 1024;
/// The most one read returns, whatever the caller asks for.
pub const MAX_READ: usize = 1024 * 1024;

const END_FILE: &str = "end.json";
const LOCK_FILE: &str = "finalize.lock";

/// Makes a run's folder and its `finalize.lock`, before the run's record names it.
pub fn create_run_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("Failed to create '{}'", dir.display()))?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOCK_FILE))
        .with_context(|| format!("Failed to create '{}'", dir.join(LOCK_FILE).display()))?;
    Ok(())
}

// --- Finalization -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndReason {
    /// Everything the run's channel carried was written.
    Complete,
    /// The reader died, so bytes still buffered in the pipe were lost.
    Interrupted,
}

/// `end.json`: the run is over, and its stream ends at `offset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEnd {
    pub reason: EndReason,
    pub offset: u64,
    /// Unix seconds, for display.
    pub at: u64,
    /// Why capture stopped, when writing the stream failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RunEnd {
    pub fn now(reason: EndReason, offset: u64) -> Self {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0);
        Self {
            reason,
            offset,
            at,
            error: None,
        }
    }
}

/// The run's `end.json`, or `None` while it is not finalized.
pub fn read_end(dir: &Path) -> Result<Option<RunEnd>> {
    let path = dir.join(END_FILE);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content)
            .map(Some)
            .with_context(|| format!("'{}' is not a valid end record", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read '{}'", path.display())),
    }
}

/// Publishes `end.json` whole: written to a unique temporary file, synced, then
/// linked to its name, which fails when an end is already there. Returns
/// whether this call published it.
pub fn publish_end(dir: &Path, end: &RunEnd) -> Result<bool> {
    let temporary = dir.join(format!(
        "{END_FILE}.{}.{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    let mut file = File::create_new(&temporary)
        .with_context(|| format!("Failed to create '{}'", temporary.display()))?;
    file.write_all(&serde_json::to_vec(end)?)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
    let linked = fs::hard_link(&temporary, dir.join(END_FILE));
    let _ = fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("Failed to publish '{}'", dir.join(END_FILE).display())),
    }
}

/// An exclusive `flock` on the run's `finalize.lock`, held for a whole
/// finalization. The kernel releases it when its holder dies, so the next
/// finisher takes over without checking anyone's identity.
pub struct FinalizeLock {
    _file: File,
}

pub enum TryFinalizeLock {
    Acquired(FinalizeLock),
    /// Another finisher holds it.
    Busy,
    /// The run's folder is gone: its service moved on or was forgotten.
    Gone,
}

impl FinalizeLock {
    /// Waits for the lock. `None` when the run's folder is gone, before or
    /// while waiting: nothing is left to finalize, and nothing is recreated.
    pub fn acquire(dir: &Path) -> Result<Option<Self>> {
        let Some(file) = open_lock_file(dir)? else {
            return Ok(None);
        };
        file.lock()
            .with_context(|| format!("Failed to lock '{}'", dir.join(LOCK_FILE).display()))?;
        Ok(dir.is_dir().then_some(Self { _file: file }))
    }

    pub fn try_acquire(dir: &Path) -> Result<TryFinalizeLock> {
        let Some(file) = open_lock_file(dir)? else {
            return Ok(TryFinalizeLock::Gone);
        };
        match file.try_lock() {
            Ok(()) if dir.is_dir() => Ok(TryFinalizeLock::Acquired(Self { _file: file })),
            Ok(()) => Ok(TryFinalizeLock::Gone),
            Err(TryLockError::WouldBlock) => Ok(TryFinalizeLock::Busy),
            Err(TryLockError::Error(error)) => Err(error)
                .with_context(|| format!("Failed to lock '{}'", dir.join(LOCK_FILE).display())),
        }
    }
}

fn open_lock_file(dir: &Path) -> Result<Option<File>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOCK_FILE))
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("Failed to open '{}'", dir.join(LOCK_FILE).display())),
    }
}

// --- Writing ------------------------------------------------------------

/// Appends to a run's segments, rotating every [`SEGMENT_SIZE`] bytes.
pub struct StreamWriter {
    dir: PathBuf,
    segment_size: u64,
    index: u64,
    length: u64,
    file: File,
}

impl StreamWriter {
    /// Starts the stream of a new run, in its folder.
    pub fn create(dir: &Path) -> Result<Self> {
        Self::with_segment_size(dir, SEGMENT_SIZE)
    }

    pub(crate) fn with_segment_size(dir: &Path, segment_size: u64) -> Result<Self> {
        let file = File::create_new(segment_path(dir, 0))
            .with_context(|| format!("Failed to start the output stream in '{}'", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            segment_size,
            index: 0,
            length: 0,
            file,
        })
    }

    pub fn append(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            if self.length >= self.segment_size {
                self.rotate()?;
            }
            let room = usize::try_from(self.segment_size - self.length).unwrap_or(usize::MAX);
            let (now, later) = bytes.split_at(room.min(bytes.len()));
            self.file.write_all(now)?;
            self.length += now.len() as u64;
            bytes = later;
        }
        Ok(())
    }

    /// Where the next byte goes.
    pub fn offset(&self) -> u64 {
        self.index * self.segment_size + self.length
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.index += 1;
        self.file = File::create_new(segment_path(&self.dir, self.index))?;
        self.length = 0;
        if let Some(dropped) = self.index.checked_sub(2) {
            let _ = fs::remove_file(segment_path(&self.dir, dropped));
        }
        Ok(())
    }
}

/// One stream shared by the copies of a service's stdout and stderr. Once
/// closed, it keeps nothing more: what a process that left the run writes
/// after its end is not the run's.
#[derive(Clone)]
pub struct StreamSink {
    state: Arc<Mutex<SinkState>>,
}

struct SinkState {
    writer: StreamWriter,
    closed: bool,
    /// The first write that failed, kept even when the copy never finishes.
    failure: Option<String>,
}

impl StreamSink {
    pub fn new(writer: StreamWriter) -> Self {
        Self {
            state: Arc::new(Mutex::new(SinkState {
                writer,
                closed: false,
                failure: None,
            })),
        }
    }

    /// Why writing failed, the first time it did.
    pub fn failure(&self) -> Option<String> {
        self.lock().failure.clone()
    }

    /// Stops writing and returns where the stream ends.
    pub fn close(&self) -> u64 {
        let mut state = self.lock();
        state.closed = true;
        state.writer.offset()
    }

    fn lock(&self) -> MutexGuard<'_, SinkState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Write for StreamSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.lock();
        if !state.closed
            && let Err(error) = state.writer.append(bytes)
        {
            state.failure.get_or_insert_with(|| error.to_string());
            return Err(error);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn segment_path(dir: &Path, index: u64) -> PathBuf {
    dir.join(format!("{index}.log"))
}

// --- Reading ------------------------------------------------------------

/// What one read returns, as `output.data` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRead {
    /// Where these lines start.
    pub offset: u64,
    /// Where to read on from.
    pub next: u64,
    pub lines: Vec<StreamLine>,
    /// The asked offset was dropped by rotation; data starts here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotated: Option<u64>,
    /// The run ended and everything kept was read.
    pub ended: bool,
    /// With `ended`: capture stopped early.
    pub interrupted: bool,
    /// With `interrupted`: why, when writing the stream failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamLine {
    pub text: String,
    /// The line goes on in the next item.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

/// Reads at most `max_bytes` from `offset` and returns them as plain lines.
/// A line still being written is held back, and `next` stops before it.
pub fn read(dir: &Path, offset: u64, max_bytes: usize) -> Result<StreamRead> {
    read_with(dir, offset, max_bytes, SEGMENT_SIZE)
}

/// How many logical bytes the stream holds, and the first one still kept.
pub fn extent(dir: &Path) -> Result<(u64, u64)> {
    let extent = Extent::of(dir, SEGMENT_SIZE)?;
    Ok((extent.first, extent.total))
}

fn read_with(dir: &Path, offset: u64, max_bytes: usize, segment_size: u64) -> Result<StreamRead> {
    // Room for one whole UTF-8 character, so every read can make progress.
    let max_bytes = max_bytes.clamp(4, MAX_READ) as u64;
    // The writer may drop a segment between the listing and the read.
    for _ in 0..3 {
        let end = read_end(dir)?;
        let extent = Extent::of(dir, segment_size)?;
        let total = end
            .as_ref()
            .map_or(extent.total, |end| end.offset.min(extent.total));
        if offset > total {
            bail!("Offset {offset} is past the {total} bytes this run has written.");
        }
        let start = offset.max(extent.first);
        let wanted = (total - start).min(max_bytes);
        match read_bytes(dir, start, wanted, segment_size) {
            Ok(bytes) => {
                let mut read = to_lines(start, &bytes, total, end.as_ref());
                read.rotated = (offset < extent.first).then_some(extent.first);
                return Ok(read);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read the output in '{}'", dir.display()));
            }
        }
    }
    bail!(
        "The output in '{}' rotated while it was read. Try again.",
        dir.display()
    )
}

struct Extent {
    first: u64,
    total: u64,
}

impl Extent {
    fn of(dir: &Path, segment_size: u64) -> Result<Self> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self { first: 0, total: 0 });
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to read '{}'", dir.display()));
            }
        };
        let indexes: Vec<u64> = entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()?
                    .strip_suffix(".log")?
                    .parse()
                    .ok()
            })
            .collect();
        let (Some(first), Some(last)) = (indexes.iter().min(), indexes.iter().max()) else {
            return Ok(Self { first: 0, total: 0 });
        };
        let last_length = match fs::metadata(segment_path(dir, *last)) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            first: first * segment_size,
            total: last * segment_size + last_length,
        })
    }
}

/// Reads `wanted` bytes from `start`, across segments, with a buffer of
/// exactly that size.
fn read_bytes(dir: &Path, start: u64, wanted: u64, segment_size: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(usize::try_from(wanted).unwrap_or(0));
    let mut position = start;
    while (bytes.len() as u64) < wanted {
        let index = position / segment_size;
        let within = position % segment_size;
        let left = (wanted - bytes.len() as u64).min(segment_size - within);
        let mut file = File::open(segment_path(dir, index))?;
        file.seek(SeekFrom::Start(within))?;
        let read = file.take(left).read_to_end(&mut bytes)?;
        if read == 0 {
            break;
        }
        position += read as u64;
    }
    Ok(bytes)
}

fn to_lines(start: u64, bytes: &[u8], total: u64, end: Option<&RunEnd>) -> StreamRead {
    let reached_total = start + bytes.len() as u64 == total;
    let mut lines = Vec::new();
    let mut consumed = 0;
    let mut rest = bytes;
    while let Some(position) = rest.iter().position(|byte| *byte == b'\n') {
        push_line(&mut lines, &rest[..position]);
        consumed += position + 1;
        rest = &rest[position + 1..];
    }
    if !rest.is_empty() {
        if end.is_some() && reached_total {
            // The run is over: its last unfinished line is sent as it is.
            push_line(&mut lines, rest);
            consumed += rest.len();
        } else {
            // A line still being written goes out only in whole 8 KiB pieces.
            while rest.len() >= MAX_LINE {
                let cut = char_boundary(rest, MAX_LINE);
                push_cut(&mut lines, &rest[..cut]);
                consumed += cut;
                rest = &rest[cut..];
            }
            // Unless the read stopped inside a line longer than it: then what
            // was read goes too, but never half a character.
            let whole = rest.len() - incomplete_char_tail(rest);
            if consumed == 0 && !reached_total && whole > 0 {
                push_cut(&mut lines, &rest[..whole]);
                consumed += whole;
            }
        }
    }
    let next = start + consumed as u64;
    let ended = end.is_some() && next == total;
    StreamRead {
        offset: start,
        next,
        lines,
        rotated: None,
        ended,
        interrupted: ended && end.is_some_and(|end| end.reason == EndReason::Interrupted),
        capture_error: end.and_then(|end| end.error.clone()).filter(|_| ended),
    }
}

/// A complete line, in pieces of at most [`MAX_LINE`] bytes.
fn push_line(lines: &mut Vec<StreamLine>, mut line: &[u8]) {
    while line.len() > MAX_LINE {
        let cut = char_boundary(line, MAX_LINE);
        push_cut(lines, &line[..cut]);
        line = &line[cut..];
    }
    lines.push(StreamLine {
        text: plain_line(&String::from_utf8_lossy(line)),
        cut: false,
    });
}

fn push_cut(lines: &mut Vec<StreamLine>, piece: &[u8]) {
    lines.push(StreamLine {
        text: plain_line(&String::from_utf8_lossy(piece)),
        cut: true,
    });
}

/// How many bytes at the end of `bytes` start a UTF-8 character that is not
/// complete yet.
fn incomplete_char_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let length = match byte {
            0b1100_0000..=0b1101_1111 => 2,
            0b1110_0000..=0b1110_1111 => 3,
            0b1111_0000..=0b1111_0111 => 4,
            _ => 1,
        };
        return if length > back { back } else { 0 };
    }
    0
}

/// Where to cut `bytes` at most `limit` bytes in, without splitting a UTF-8
/// character when one ends shortly before.
fn char_boundary(bytes: &[u8], limit: usize) -> usize {
    if bytes.len() <= limit {
        // Up to the limit exactly, the last character may still be cut.
        let whole = bytes.len() - incomplete_char_tail(bytes);
        return if whole > 0 { whole } else { bytes.len() };
    }
    let floor = limit.saturating_sub(3).max(1);
    let mut cut = limit;
    while cut > floor && bytes[cut] & 0b1100_0000 == 0b1000_0000 {
        cut -= 1;
    }
    if bytes[cut] & 0b1100_0000 == 0b1000_0000 {
        limit
    } else {
        cut
    }
}

// --- Plain lines ----------------------------------------------------------

/// Terminal output as plain lines: colors and cursor moves removed, and a line
/// redrawn with `\r` (progress bars) kept as its last state.
pub fn plain_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(plain_line).collect();
    // A text that ends with a line break has no line after it.
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// One line as plain text: escape sequences, control characters other than
/// tab (C1 included) and bidirectional overrides removed.
pub fn plain_line(raw: &str) -> String {
    let raw = raw.strip_suffix('\r').unwrap_or(raw);
    let mut line = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                // Back to the line's start (spinners): what follows replaces it.
                if skip_escape(&mut chars) == Some('G') {
                    line.clear();
                }
            }
            '\r' => line.clear(),
            '\u{8}' => {
                line.pop();
            }
            '\t' => line.push(ch),
            ch if ch.is_control() || is_bidi_override(ch) => {}
            ch => line.push(ch),
        }
    }
    line
}

fn is_bidi_override(ch: char) -> bool {
    matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Skips one escape sequence. Returns the final byte of a CSI sequence.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    match chars.next() {
        // CSI: parameters, then one final byte in @..~.
        Some('[') => chars.by_ref().find(|ch| ('@'..='~').contains(ch)),
        // OSC (titles, links): ends with BEL or ESC \.
        Some(']') => {
            while let Some(ch) = chars.next() {
                if ch == '\u{7}' {
                    break;
                }
                if ch == '\u{1b}' {
                    chars.next();
                    break;
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(segment_size: u64) -> (tempfile::TempDir, StreamWriter) {
        let dir = tempfile::tempdir().unwrap();
        create_run_dir(dir.path()).unwrap();
        let writer = StreamWriter::with_segment_size(dir.path(), segment_size).unwrap();
        (dir, writer)
    }

    fn texts(read: &StreamRead) -> Vec<&str> {
        read.lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn output_reads_as_plain_lines() {
        let text = "\u{1b}[32mready\u{1b}[0m in 2s\r\n\u{1b}]0;title\u{7}50%\r100%\n⠙\u{1b}[1G\u{1b}[0Klast";

        assert_eq!(plain_lines(text), ["ready in 2s", "100%", "last"]);
        assert_eq!(plain_lines("a\n\nb\n"), ["a", "", "b"]);
    }

    #[test]
    fn control_characters_and_bidi_overrides_are_removed() {
        assert_eq!(
            plain_line("ok\u{202E}evil\u{2066}\u{85}\u{7}\tdone"),
            "okevil\tdone"
        );
    }

    #[test]
    fn a_line_still_being_written_is_held_back() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(b"one\ntwo\nthr").unwrap();

        let first = read(dir.path(), 0, 4096).unwrap();
        assert_eq!(texts(&first), ["one", "two"]);
        assert_eq!(first.next, 8);
        assert!(!first.ended);

        writer.append(b"ee\n").unwrap();
        let next = read(dir.path(), first.next, 4096).unwrap();
        assert_eq!(texts(&next), ["three"]);
        assert_eq!(read(dir.path(), next.next, 4096).unwrap().lines, []);
    }

    #[test]
    fn reads_are_bounded_and_go_on_from_next() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        for index in 0..100 {
            writer.append(format!("line {index}\n").as_bytes()).unwrap();
        }

        let mut offset = 0;
        let mut seen = Vec::new();
        loop {
            let chunk = read(dir.path(), offset, 64).unwrap();
            assert!(chunk.next - chunk.offset <= 64);
            seen.extend(chunk.lines.into_iter().map(|line| line.text));
            if chunk.next == offset {
                break;
            }
            offset = chunk.next;
        }
        assert_eq!(seen.len(), 100);
        assert_eq!(seen[99], "line 99");
    }

    #[test]
    fn a_long_line_comes_in_cut_pieces() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(&vec![b'x'; MAX_LINE * 2 + 10]).unwrap();

        let open = read(dir.path(), 0, MAX_READ).unwrap();
        assert_eq!(open.lines.len(), 2);
        assert!(open.lines.iter().all(|line| line.cut));
        assert_eq!(open.next, (MAX_LINE * 2) as u64);

        writer.append(b"\n").unwrap();
        let rest = read(dir.path(), open.next, MAX_READ).unwrap();
        assert_eq!(
            rest.lines,
            [StreamLine {
                text: "x".repeat(10),
                cut: false
            }]
        );
    }

    #[test]
    fn a_read_smaller_than_a_line_still_makes_progress() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(&[b'y'; 100]).unwrap();
        writer.append(b"\n").unwrap();

        let chunk = read(dir.path(), 0, 40).unwrap();

        assert_eq!(chunk.next, 40);
        assert!(chunk.lines[0].cut);
    }

    #[test]
    fn a_character_across_the_read_boundary_waits_for_the_next_read() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append("abcé and more".as_bytes()).unwrap();

        // 4 bytes stop in the middle of "é", 2 bytes in UTF-8.
        let first = read(dir.path(), 0, 4).unwrap();
        assert_eq!(texts(&first), ["abc"]);
        assert_eq!(first.next, 3);

        let second = read(dir.path(), first.next, 4).unwrap();
        assert_eq!(texts(&second)[0], "é a");
        assert!(
            second
                .lines
                .iter()
                .all(|line| !line.text.contains('\u{FFFD}'))
        );
    }

    #[test]
    fn a_character_at_exactly_8_kib_is_never_split() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(&vec![b'a'; MAX_LINE - 1]).unwrap();
        writer.append("é".as_bytes()).unwrap();

        // The read ends exactly at 8 KiB, inside "é".
        let first = read(dir.path(), 0, MAX_LINE).unwrap();
        assert_eq!(first.next, (MAX_LINE - 1) as u64);
        assert!(
            first
                .lines
                .iter()
                .all(|line| !line.text.contains('\u{FFFD}'))
        );

        writer.append(b"\n").unwrap();
        let second = read(dir.path(), first.next, MAX_LINE).unwrap();
        assert_eq!(second.next, (MAX_LINE + 2) as u64);
        assert_eq!(second.lines[0].text, "é");
    }

    #[test]
    fn a_megabyte_line_without_a_newline_comes_in_8_kib_pieces() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(&vec![b'z'; MAX_READ + 10]).unwrap();

        let chunk = read(dir.path(), 0, MAX_READ).unwrap();

        assert_eq!(chunk.next, MAX_READ as u64);
        assert_eq!(chunk.lines.len(), MAX_READ / MAX_LINE);
        assert!(
            chunk
                .lines
                .iter()
                .all(|line| line.cut && line.text.len() <= MAX_LINE)
        );
    }

    #[test]
    fn rotation_keeps_the_two_newest_segments_and_says_what_was_dropped() {
        let (dir, mut writer) = stream(16);
        for _ in 0..10 {
            writer.append(b"0123456789abcde\n").unwrap();
        }
        assert_eq!(writer.offset(), 160);
        assert!(!segment_path(dir.path(), 7).exists());
        assert!(segment_path(dir.path(), 8).exists());

        let chunk = read_with(dir.path(), 0, 1024, 16).unwrap();
        assert_eq!(chunk.rotated, Some(128));
        assert_eq!(chunk.offset, 128);
        assert_eq!(texts(&chunk), ["0123456789abcde", "0123456789abcde"]);
        assert_eq!(chunk.next, 160);
    }

    #[test]
    fn an_offset_past_the_end_is_an_error() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(b"one\n").unwrap();

        assert!(read(dir.path(), 5, 1024).is_err());
    }

    #[test]
    fn an_ended_stream_sends_its_last_line_and_says_how_it_ended() {
        let (dir, mut writer) = stream(SEGMENT_SIZE);
        writer.append(b"done\nno newline").unwrap();
        let end = RunEnd::now(EndReason::Interrupted, writer.offset());
        assert!(publish_end(dir.path(), &end).unwrap());
        assert!(!publish_end(dir.path(), &end).unwrap());

        let chunk = read(dir.path(), 0, 1024).unwrap();

        assert_eq!(texts(&chunk), ["done", "no newline"]);
        assert!(chunk.ended && chunk.interrupted);
        assert_eq!(read_end(dir.path()).unwrap(), Some(end));
    }

    #[test]
    fn a_closed_sink_keeps_nothing_more() {
        let (dir, writer) = stream(SEGMENT_SIZE);
        let mut sink = StreamSink::new(writer);
        sink.write_all(b"kept\n").unwrap();

        assert_eq!(sink.close(), 5);
        sink.write_all(b"after the end\n").unwrap();

        assert_eq!(extent(dir.path()).unwrap(), (0, 5));
    }

    #[test]
    fn the_finalize_lock_is_exclusive_and_never_recreates_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        create_run_dir(&run).unwrap();

        let held = FinalizeLock::acquire(&run).unwrap().unwrap();
        assert!(matches!(
            FinalizeLock::try_acquire(&run).unwrap(),
            TryFinalizeLock::Busy
        ));
        drop(held);
        assert!(matches!(
            FinalizeLock::try_acquire(&run).unwrap(),
            TryFinalizeLock::Acquired(_)
        ));

        fs::remove_dir_all(&run).unwrap();
        assert!(FinalizeLock::acquire(&run).unwrap().is_none());
        assert!(!run.exists());
    }

    #[test]
    fn cuts_never_split_a_character() {
        let mut bytes = vec![b'a'; MAX_LINE - 1];
        bytes.extend_from_slice("é".as_bytes());

        assert_eq!(char_boundary(&bytes, MAX_LINE), MAX_LINE - 1);
    }
}
