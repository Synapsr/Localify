//! The memory gate of docs/remote-machines.md, PR 1: a read of a 1 GiB
//! stream, and of a 1 GiB line, never holds more than `maxBytes` plus a
//! constant. A counting allocator measures the peak each read reaches. The
//! data is written on the fly through the real writer, which keeps two 4 MiB
//! segments, so the disk never holds more than 8 MiB of it.
//!
//! It writes and reads 2 GiB, too slow for every CI run:
//! `cargo test --test output_memory -- --ignored`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use localify::infrastructure::runs::output::{self, StreamWriter};

/// Counts the bytes held, and the most held since the last [`reset_peak`].
struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to the system allocator unchanged; the counters
// only observe the sizes it was given.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract, passed on as is.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let held = HELD.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(held, Ordering::SeqCst);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` came from `alloc` above with this `layout`.
        unsafe { System.dealloc(pointer, layout) };
        HELD.fetch_sub(layout.size(), Ordering::SeqCst);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const GIB: u64 = 1024 * 1024 * 1024;
/// What one write appends: the stream grows by this much between reads.
const CHUNK: usize = 1024 * 1024;
/// The most an `output.read` may ask for.
const MAX_BYTES: usize = 16 * 1024;
/// What a read may hold beyond `maxBytes`: the plain lines it returns, their
/// list, and the segment it opened.
const CONSTANT: usize = 64 * 1024;

/// Bytes held while `read` runs and by what it returns, beyond what was held
/// before.
fn peak_of<T>(read: impl FnOnce() -> T) -> usize {
    let before = HELD.load(Ordering::SeqCst);
    PEAK.store(before, Ordering::SeqCst);
    let result = read();
    let peak = PEAK.load(Ordering::SeqCst);
    drop(result);
    peak.saturating_sub(before)
}

/// Appends `GIB` bytes made of `chunk`, reading everything new after each
/// write the way `output.follow` does, `MAX_BYTES` at a time. Returns the
/// highest peak a read reached, and how many bytes the reads covered.
fn follow_a_gibibyte(dir: &Path, chunk: &[u8]) -> (usize, u64) {
    let mut writer = StreamWriter::create(dir).unwrap();
    let (mut offset, mut highest) = (0, 0);
    while writer.offset() < GIB {
        writer.append(chunk).unwrap();
        loop {
            let mut next = offset;
            let peak = peak_of(|| {
                let read = output::read(dir, offset, MAX_BYTES).unwrap();
                next = read.next;
                read
            });
            highest = highest.max(peak);
            if next == offset {
                break;
            }
            offset = next;
        }
    }
    (highest, offset)
}

fn run_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    output::create_run_dir(dir.path()).unwrap();
    dir
}

#[test]
#[ignore = "writes and reads 2 GiB; run with --ignored"]
fn reads_of_a_gibibyte_stream_and_a_gibibyte_line_stay_bounded() {
    // Ordinary output: lines of 100 bytes.
    let mut lines = Vec::with_capacity(CHUNK);
    while lines.len() < CHUNK {
        lines.extend_from_slice(&[b'a'; 99]);
        lines.push(b'\n');
    }
    lines.truncate(CHUNK - CHUNK % 100);
    let dir = run_dir();
    let (stream_peak, stream_read) = follow_a_gibibyte(dir.path(), &lines);
    drop(dir);

    // One line that never ends.
    let dir = run_dir();
    let (line_peak, line_read) = follow_a_gibibyte(dir.path(), &vec![b'x'; CHUNK]);
    drop(dir);

    eprintln!(
        "1 GiB stream: {stream_read} bytes read, peak {stream_peak} bytes per read; \
         1 GiB line: {line_read} bytes read, peak {line_peak} bytes per read; \
         bound {} bytes",
        MAX_BYTES + CONSTANT
    );
    assert!(stream_read >= GIB && line_read >= GIB);
    // The counter sees each read's own buffer, so it measures something.
    assert!(stream_peak >= MAX_BYTES && line_peak >= MAX_BYTES);
    assert!(stream_peak <= MAX_BYTES + CONSTANT, "{stream_peak}");
    assert!(line_peak <= MAX_BYTES + CONSTANT, "{line_peak}");
}
