//! One send queue per relay socket, paced below the relay's budget
//! (`docs/remote-machines.md`, "Limits", "Send pacing"): at most 40 frames
//! and 384 KiB a second for all sessions together, whatever role this
//! machine plays in each. Bursts stay at 8 frames and 128 KiB. The relay
//! lets 32 frames or 512 KiB wait behind one it is checking against D1, so
//! the pacer stays inside that while a check lasts up to 600 ms, and inside
//! its buckets (200 frames at 50 a second, 5 MiB at 512 KiB a second) at
//! all times.
//!
//! The queue in front of the pacer holds at most 24 frames and 256 KiB. A
//! daemon that cannot hand over a frame treats the socket as stuck: the
//! connection ends and the link reconnects. A write or a close that takes
//! longer than 10 s ends the writer the same way.
//!
//! The queue itself is first-in first-out, since each session's messages
//! must reach the wire in the order they were encrypted. Priority comes
//! from what is let in: handshakes, heartbeats, pairing and errors go in at
//! once, while inventory pages and output wait in the actor and go in only
//! while the queue holds less than [`BULK_FRAMES`] and [`BULK_BYTES`], so
//! the others always find room, and never wait behind more than that. The
//! writer says when it took a frame, so the actor tops the queue up.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{Sink, SinkExt};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

pub const FRAMES_PER_SECOND: f64 = 40.0;
pub const BYTES_PER_SECOND: f64 = 384.0 * 1024.0;
pub const BURST_FRAMES: f64 = 8.0;
pub const BURST_BYTES: f64 = 128.0 * 1024.0;
pub const QUEUE_FRAMES: usize = 24;
pub const QUEUE_BYTES: usize = 256 * 1024;
/// Inventory pages and output go in only below these.
pub const BULK_FRAMES: usize = 6;
pub const BULK_BYTES: usize = 96 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The sending end of a socket's queue.
pub struct Outbox {
    sender: mpsc::Sender<String>,
    queued: Arc<AtomicUsize>,
    drained: Arc<Notify>,
}

/// The writer's end.
pub struct Queue {
    receiver: mpsc::Receiver<String>,
    queued: Arc<AtomicUsize>,
    drained: Arc<Notify>,
}

/// The queue of one socket, bounded in frames and in bytes.
pub fn queue() -> (Outbox, Queue) {
    let (sender, receiver) = mpsc::channel(QUEUE_FRAMES);
    let queued = Arc::new(AtomicUsize::new(0));
    let drained = Arc::new(Notify::new());
    let outbox = Outbox {
        sender,
        queued: Arc::clone(&queued),
        drained: Arc::clone(&drained),
    };
    let queue = Queue {
        receiver,
        queued,
        drained,
    };
    (outbox, queue)
}

impl Outbox {
    /// Whether an inventory page or output may go in now.
    pub fn has_room_for_bulk(&self) -> bool {
        let frames = QUEUE_FRAMES - self.sender.capacity();
        frames < BULK_FRAMES && self.queued.load(Ordering::SeqCst) < BULK_BYTES
    }

    /// Notified each time the writer takes a frame.
    pub fn drained(&self) -> Arc<Notify> {
        Arc::clone(&self.drained)
    }

    /// Queues a frame; `false` when the queue is full or the writer gone.
    pub fn push(&self, text: String) -> bool {
        let size = text.len();
        if self.queued.fetch_add(size, Ordering::SeqCst) + size > QUEUE_BYTES {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            return false;
        }
        if self.sender.try_send(text).is_err() {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// A token bucket for frames and bytes, holding at most one burst.
#[derive(Debug, Clone)]
pub struct Bucket {
    frames: f64,
    bytes: f64,
    frame_rate: f64,
    byte_rate: f64,
    frame_burst: f64,
    byte_burst: f64,
    updated: Instant,
}

impl Bucket {
    /// The daemon's pacing: the rates and bursts above.
    pub fn paced(now: Instant) -> Self {
        Self {
            frames: BURST_FRAMES,
            bytes: BURST_BYTES,
            frame_rate: FRAMES_PER_SECOND,
            byte_rate: BYTES_PER_SECOND,
            frame_burst: BURST_FRAMES,
            byte_burst: BURST_BYTES,
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.updated = now;
        self.frames = (self.frames + elapsed * self.frame_rate).min(self.frame_burst);
        self.bytes = (self.bytes + elapsed * self.byte_rate).min(self.byte_burst);
    }

    /// How long to wait before a frame of `size` bytes may go. A frame
    /// larger than a burst waits until the bucket is full.
    pub fn wait_for(&mut self, size: usize, now: Instant) -> Duration {
        self.refill(now);
        let size = (size as f64).min(self.byte_burst);
        let frames = ((1.0 - self.frames) / self.frame_rate).max(0.0);
        let bytes = ((size - self.bytes) / self.byte_rate).max(0.0);
        Duration::from_secs_f64(frames.max(bytes))
    }

    /// Takes a frame's tokens; call once [`Bucket::wait_for`] is zero.
    pub fn take(&mut self, size: usize, now: Instant) {
        self.refill(now);
        self.frames -= 1.0;
        self.bytes -= (size as f64).min(self.byte_burst);
    }
}

/// Writes every queued text frame to the socket, paced. Returns once the
/// queue is closed, the socket fails, or a write or the close takes longer
/// than 10 s.
pub async fn run<S>(mut sink: S, mut queue: Queue, mut bucket: Bucket)
where
    S: Sink<Message> + Unpin,
{
    while let Some(text) = queue.receiver.recv().await {
        let size = text.len();
        queue.queued.fetch_sub(size, Ordering::SeqCst);
        queue.drained.notify_one();
        let wait = bucket.wait_for(size, Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        bucket.take(size, Instant::now());
        match tokio::time::timeout(WRITE_TIMEOUT, sink.send(Message::text(text))).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                debug!("The relay socket closed while sending");
                return;
            }
            Err(_) => {
                warn!("A write to the relay took more than 10 s; dropping the connection");
                return;
            }
        }
    }
    let _ = tokio::time::timeout(WRITE_TIMEOUT, sink.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_goes_at_once_then_frames_wait_their_turn() {
        let start = Instant::now();
        let mut bucket = Bucket::paced(start);
        for _ in 0..8 {
            assert_eq!(bucket.wait_for(100, start), Duration::ZERO);
            bucket.take(100, start);
        }
        let wait = bucket.wait_for(100, start);
        assert!((wait.as_secs_f64() - 0.025).abs() < 1e-6, "{wait:?}");
    }

    #[test]
    fn ten_seconds_of_sending_stays_under_the_relays_budget() {
        // Every frame sent as soon as the bucket allows, for 10 s.
        let start = Instant::now();
        let mut now = start;
        let mut bucket = Bucket::paced(start);
        let (mut frames, mut bytes) = (0_u32, 0_usize);
        while now < start + Duration::from_secs(10) {
            now += bucket.wait_for(2048, now);
            bucket.take(2048, now);
            frames += 1;
            bytes += 2048;
        }
        // The relay allows 50 frames and 512 KiB a second over 10 s.
        assert!(frames <= 10 * 40 + 8, "{frames}");
        assert!(bytes <= 10 * 384 * 1024 + 128 * 1024, "{bytes}");
        assert!(frames < 500 && bytes < 10 * 512 * 1024);
    }

    #[test]
    fn large_frames_are_paced_by_bytes() {
        let start = Instant::now();
        let mut bucket = Bucket::paced(start);
        bucket.take(60 * 1024, start);
        let mut now = start;
        let mut sent = 1;
        while now < start + Duration::from_secs(1) {
            now += bucket.wait_for(60 * 1024, now);
            bucket.take(60 * 1024, now);
            sent += 1;
        }
        // 384 KiB a second plus a 128 KiB burst: 8 frames of 60 KiB.
        assert!(sent <= 9, "{sent}");
    }

    /// A sink that never takes a frame, as a socket whose peer stopped
    /// reading.
    struct Stuck;

    impl Sink<Message> for Stuck {
        type Error = std::io::Error;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }
        fn start_send(self: std::pin::Pin<&mut Self>, _: Message) -> Result<(), Self::Error> {
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_writer_stuck_on_its_socket_gives_up_after_the_write_timeout() {
        let (outbox, queue) = queue();
        assert!(outbox.push("ping".into()));
        let started = Instant::now();
        run(Stuck, queue, Bucket::paced(started)).await;
        assert_eq!(started.elapsed(), WRITE_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closing_writer_gives_up_after_the_write_timeout() {
        let (outbox, queue) = queue();
        drop(outbox);
        let started = Instant::now();
        run(Stuck, queue, Bucket::paced(started)).await;
        assert_eq!(started.elapsed(), WRITE_TIMEOUT);
    }

    #[test]
    fn bulk_frames_leave_room_for_the_others() {
        let (outbox, _queue) = queue();
        for _ in 0..BULK_FRAMES {
            assert!(outbox.has_room_for_bulk());
            assert!(outbox.push("x".into()));
        }
        assert!(!outbox.has_room_for_bulk());
        // A heartbeat still goes in.
        assert!(outbox.push("keep".into()));

        let (outbox, _queue) = queue();
        assert!(outbox.push("x".repeat(BULK_BYTES)));
        assert!(!outbox.has_room_for_bulk());
    }

    #[test]
    fn the_queue_is_bounded_in_frames_and_bytes() {
        let (outbox, _queue) = queue();
        for _ in 0..QUEUE_FRAMES {
            assert!(outbox.push("x".into()));
        }
        assert!(!outbox.push("x".into()));

        let (outbox, _queue) = queue();
        assert!(outbox.push("x".repeat(QUEUE_BYTES - 10)));
        assert!(!outbox.push("x".repeat(11)));
        assert!(outbox.push("x".repeat(10)));
    }
}
