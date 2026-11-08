//! Masks secret values in a service's stdout and stderr.
//!
//! Best effort: a value the service transforms (encodes, splits, escapes) is
//! not recognized. Short values are never masked, because masking `true` or
//! `3000` would garble every log line.

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const MASK: &[u8] = b"***";

/// Values shorter than this are not worth masking.
pub const MIN_MASKED_LENGTH: usize = 8;

/// Keys whose values end up in the browser bundle anyway.
const PUBLIC_KEY_PREFIXES: &[&str] = &["NEXT_PUBLIC_", "VITE_", "PUBLIC_", "EXPO_PUBLIC_"];

/// How long a partial match may be held back before it is printed.
const IDLE_FLUSH: Duration = Duration::from_millis(150);
/// The most one read of a service's output takes.
pub const CHUNK: usize = 64 * 1024;
/// How many read chunks may wait for the writer.
pub const QUEUED_CHUNKS: usize = 2;

/// Picks the byte patterns to mask from secret variables. Multi-line values
/// are also masked line by line, since services often log one line of them.
pub fn secret_patterns<'a>(secrets: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Vec<u8>> {
    let mut patterns: Vec<Vec<u8>> = Vec::new();
    for (key, value) in secrets {
        if PUBLIC_KEY_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
        {
            continue;
        }
        let pieces = std::iter::once(value).chain(value.lines().map(str::trim));
        for piece in pieces {
            if piece.len() >= MIN_MASKED_LENGTH && !patterns.iter().any(|p| p == piece.as_bytes()) {
                patterns.push(piece.as_bytes().to_vec());
            }
        }
    }
    patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.len()));
    patterns
}

/// Streaming masker that holds back only bytes that could still turn into a
/// secret, so a value split across two writes is still masked.
pub struct Masker {
    patterns: Vec<Vec<u8>>,
    pending: Vec<u8>,
}

impl Masker {
    pub fn new(patterns: Vec<Vec<u8>>) -> Self {
        Self {
            patterns,
            pending: Vec::new(),
        }
    }

    /// Adds output and returns what is safe to print now.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        self.drain(false)
    }

    /// Returns everything held back, at end of stream or after a pause.
    pub fn flush(&mut self) -> Vec<u8> {
        self.drain(true)
    }

    fn drain(&mut self, flush: bool) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.pending.len());
        let mut index = 0;

        'scan: while index < self.pending.len() {
            let tail = &self.pending[index..];
            for pattern in &self.patterns {
                if tail.starts_with(pattern) {
                    output.extend_from_slice(MASK);
                    index += pattern.len();
                    continue 'scan;
                }
            }
            let may_become_secret = self
                .patterns
                .iter()
                .any(|pattern| pattern.len() > tail.len() && pattern.starts_with(tail));
            if may_become_secret && !flush {
                break;
            }
            output.push(self.pending[index]);
            index += 1;
        }

        self.pending.drain(..index);
        output
    }
}

/// Copies `reader` to `writer` through a `Masker` on a background thread.
/// Held-back bytes are printed after a short pause so prompts and progress
/// output are not delayed. At most [`QUEUED_CHUNKS`] chunks of [`CHUNK`] bytes
/// wait between the reader and the writer, so a slow writer slows the service
/// down instead of filling memory. Without patterns, bytes are copied as read.
///
/// When the writer fails (a full disk), the copy keeps reading the service's
/// output to its end and drops it, so the service never blocks on a full pipe
/// and never gets a SIGPIPE it did not cause, and the copy ends with the error.
pub fn spawn_masked_copy<R, W>(
    reader: R,
    mut writer: W,
    patterns: Vec<Vec<u8>>,
) -> JoinHandle<io::Result<()>>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    if patterns.is_empty() {
        return thread::spawn(move || copy_directly(reader, writer));
    }
    thread::spawn(move || {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(QUEUED_CHUNKS);
        let reader_thread = thread::spawn(move || read_chunks(reader, sender));
        let mut masker = Masker::new(patterns);

        let written = loop {
            let output = match receiver.recv_timeout(IDLE_FLUSH) {
                Ok(chunk) => masker.push(&chunk),
                Err(RecvTimeoutError::Timeout) => masker.flush(),
                Err(RecvTimeoutError::Disconnected) => {
                    break write_all(&mut writer, &masker.flush());
                }
            };
            if let Err(error) = write_all(&mut writer, &output) {
                break Err(error);
            }
        };
        // The reader's next send fails, and it drains the rest itself.
        drop(receiver);
        let _ = reader_thread.join();
        written
    })
}

fn copy_directly<R: Read, W: Write>(mut reader: R, mut writer: W) -> io::Result<()> {
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if let Err(error) = write_all(&mut writer, &buffer[..count]) {
                    drain(&mut reader, &mut buffer);
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Ok(()),
        }
    }
}

fn read_chunks<R: Read>(mut reader: R, sender: mpsc::SyncSender<Vec<u8>>) {
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                if sender.send(buffer[..count].to_vec()).is_err() {
                    drain(&mut reader, &mut buffer);
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

/// Reads to the end and keeps nothing.
fn drain<R: Read>(reader: &mut R, buffer: &mut [u8]) {
    loop {
        match reader.read(buffer) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    writer.write_all(bytes)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    /// How far the reader got, how far the writer got, and the widest gap seen.
    #[derive(Default)]
    struct Progress {
        produced: AtomicUsize,
        written: AtomicUsize,
        widest: AtomicUsize,
    }

    /// A service flooding its output: `left` bytes, as fast as they are read.
    struct Flood {
        left: usize,
        progress: Arc<Progress>,
    }

    impl Read for Flood {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.left);
            buffer[..count].fill(b'a');
            self.left -= count;
            let produced = self.progress.produced.fetch_add(count, Ordering::SeqCst) + count;
            let gap = produced - self.progress.written.load(Ordering::SeqCst);
            self.progress.widest.fetch_max(gap, Ordering::SeqCst);
            Ok(count)
        }
    }

    /// A sink much slower than the service.
    struct SlowSink(Arc<Progress>);

    impl Write for SlowSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            thread::sleep(Duration::from_millis(1));
            self.0.written.fetch_add(bytes.len(), Ordering::SeqCst);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn flood_both_outputs(patterns: Vec<Vec<u8>>) -> Arc<Progress> {
        let progress = Arc::new(Progress::default());
        let total = 8 * 1024 * 1024;
        let copies: Vec<_> = (0..2)
            .map(|_| {
                let flood = Flood {
                    left: total,
                    progress: Arc::clone(&progress),
                };
                spawn_masked_copy(flood, SlowSink(Arc::clone(&progress)), patterns.clone())
            })
            .collect();
        let started = Instant::now();
        for copy in copies {
            copy.join().unwrap().unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(60), "too slow");
        assert_eq!(progress.written.load(Ordering::SeqCst), 2 * total);
        progress
    }

    #[test]
    fn a_slow_writer_holds_back_a_flooding_service_with_masking() {
        let progress = flood_both_outputs(vec![b"supersecret".to_vec()]);

        // Per copy: the queue, the reader's buffer, the masker and the write.
        let bound = 2 * (QUEUED_CHUNKS + 4) * CHUNK;
        let widest = progress.widest.load(Ordering::SeqCst);
        assert!(widest <= bound, "{widest} bytes queued, more than {bound}");
    }

    /// A disk that fills up after `room` bytes.
    struct FullDisk {
        room: Arc<AtomicUsize>,
    }

    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let left = self.room.load(Ordering::SeqCst);
            if left < bytes.len() {
                return Err(io::Error::other("No space left on device"));
            }
            self.room.fetch_sub(bytes.len(), Ordering::SeqCst);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Both outputs keep writing after the sink fails: every copy still
    /// reaches the end of its output, so the service never blocks, and each
    /// reports the failure.
    fn flood_a_full_disk(patterns: Vec<Vec<u8>>) {
        let progress = Arc::new(Progress::default());
        let room = Arc::new(AtomicUsize::new(256 * 1024));
        let total = 4 * 1024 * 1024;
        let (done, finished) = mpsc::channel();
        for _ in 0..2 {
            let flood = Flood {
                left: total,
                progress: Arc::clone(&progress),
            };
            let sink = FullDisk {
                room: Arc::clone(&room),
            };
            let copy = spawn_masked_copy(flood, sink, patterns.clone());
            let done = done.clone();
            thread::spawn(move || done.send(copy.join().unwrap()));
        }

        for _ in 0..2 {
            let result = finished
                .recv_timeout(Duration::from_secs(20))
                .expect("a copy blocked after the disk filled up");
            assert!(result.is_err(), "the failure was not reported");
        }
        assert_eq!(progress.produced.load(Ordering::SeqCst), 2 * total);
    }

    #[test]
    fn a_full_disk_neither_blocks_the_service_nor_goes_unreported_with_masking() {
        flood_a_full_disk(vec![b"supersecret".to_vec()]);
    }

    #[test]
    fn a_full_disk_neither_blocks_the_service_nor_goes_unreported_without_masking() {
        flood_a_full_disk(Vec::new());
    }

    #[test]
    fn a_slow_writer_holds_back_a_flooding_service_without_masking() {
        let progress = flood_both_outputs(Vec::new());

        let bound = 2 * 2 * CHUNK;
        let widest = progress.widest.load(Ordering::SeqCst);
        assert!(widest <= bound, "{widest} bytes queued, more than {bound}");
    }

    fn masker(secrets: &[(&str, &str)]) -> Masker {
        Masker::new(secret_patterns(secrets.iter().copied()))
    }

    #[test]
    fn masks_secrets_in_a_single_chunk() {
        let mut masker = masker(&[("DATABASE_URL", "postgres://user:pw@db/prod")]);

        let output = masker.push(b"connecting to postgres://user:pw@db/prod ...\n");

        assert_eq!(output, b"connecting to *** ...\n");
    }

    #[test]
    fn masks_a_secret_split_across_chunks() {
        let mut masker = masker(&[("API_KEY", "sk_live_abcdef")]);

        let mut output = masker.push(b"key=sk_live_");
        assert_eq!(output, b"key=");
        output.extend(masker.push(b"abcdef done"));
        output.extend(masker.flush());

        assert_eq!(output, b"key=*** done");
    }

    #[test]
    fn prints_held_bytes_when_the_secret_does_not_follow() {
        let mut masker = masker(&[("API_KEY", "sk_live_abcdef")]);

        let mut output = masker.push(b"prefix sk_li");
        output.extend(masker.push(b"ne ok"));

        assert_eq!(output, b"prefix sk_line ok");
    }

    #[test]
    fn ignores_short_and_public_values() {
        let patterns = secret_patterns([
            ("FLAG", "true"),
            ("NEXT_PUBLIC_API_URL", "https://api.example.com"),
            ("SECRET", "long-enough-secret"),
        ]);

        assert_eq!(patterns, vec![b"long-enough-secret".to_vec()]);
    }

    #[test]
    fn masks_lines_of_multi_line_values() {
        let patterns = secret_patterns([(
            "PRIVATE_KEY",
            "-----BEGIN KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END KEY-----",
        )]);
        let mut masker = Masker::new(patterns);

        let output = masker.push(b"got MIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n");

        assert_eq!(output, b"got ***\n");
    }

    #[test]
    fn masked_copy_streams_through_a_thread() {
        let input = std::io::Cursor::new(b"token tok_1234567890 end\n".to_vec());
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = SharedWriter(output.clone());

        spawn_masked_copy(
            input,
            writer,
            secret_patterns([("TOKEN", "tok_1234567890")]),
        )
        .join()
        .unwrap()
        .unwrap();

        assert_eq!(output.lock().unwrap().as_slice(), b"token *** end\n");
    }

    struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
