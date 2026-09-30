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
/// output are not delayed.
pub fn spawn_masked_copy<R, W>(reader: R, mut writer: W, patterns: Vec<Vec<u8>>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    thread::spawn(move || {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let reader_thread = thread::spawn(move || read_chunks(reader, sender));
        let mut masker = Masker::new(patterns);

        loop {
            let output = match receiver.recv_timeout(IDLE_FLUSH) {
                Ok(chunk) => masker.push(&chunk),
                Err(RecvTimeoutError::Timeout) => masker.flush(),
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = write_all(&mut writer, &masker.flush());
                    break;
                }
            };
            if write_all(&mut writer, &output).is_err() {
                break;
            }
        }
        let _ = reader_thread.join();
    })
}

fn read_chunks<R: Read>(mut reader: R, sender: mpsc::Sender<Vec<u8>>) {
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                if sender.send(buffer[..count].to_vec()).is_err() {
                    return;
                }
            }
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
    use super::*;

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
