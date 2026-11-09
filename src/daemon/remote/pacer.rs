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
//!
//! Session messages carry their session, so the ones still queued can be
//! discarded when a target stops sharing, and [`Outbox::flushed`] is a
//! barrier: it resolves once every frame queued before it was written or
//! discarded.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{Sink, SinkExt};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::infrastructure::remote::noise::SessionId;

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

/// The session a frame seals a message of: the peer and the session id.
pub type Tag = (String, SessionId);

/// One queued frame.
struct Frame {
    text: String,
    tag: Option<Tag>,
}

/// The session messages still queued, by session, and the sessions whose
/// queued messages are discarded.
#[derive(Default)]
struct Ledger {
    pending: HashMap<Tag, usize>,
    discarded: HashSet<Tag>,
}

/// The sending end of a socket's queue.
pub struct Outbox {
    sender: mpsc::Sender<Frame>,
    queued: Arc<AtomicUsize>,
    drained: Arc<Notify>,
    ledger: Arc<Mutex<Ledger>>,
    /// Frames queued so far, and frames the writer is done with.
    pushed: AtomicU64,
    done: watch::Receiver<u64>,
}

/// The writer's end.
pub struct Queue {
    receiver: mpsc::Receiver<Frame>,
    queued: Arc<AtomicUsize>,
    drained: Arc<Notify>,
    ledger: Arc<Mutex<Ledger>>,
    done: watch::Sender<u64>,
    /// While set, the writer takes frames but sends none, as a socket that
    /// stopped taking them.
    #[cfg(test)]
    pub hold: Option<Arc<std::sync::atomic::AtomicBool>>,
}

/// The queue of one socket, bounded in frames and in bytes.
pub fn queue() -> (Outbox, Queue) {
    let (sender, receiver) = mpsc::channel(QUEUE_FRAMES);
    let queued = Arc::new(AtomicUsize::new(0));
    let drained = Arc::new(Notify::new());
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let (done, done_receiver) = watch::channel(0);
    let outbox = Outbox {
        sender,
        queued: Arc::clone(&queued),
        drained: Arc::clone(&drained),
        ledger: Arc::clone(&ledger),
        pushed: AtomicU64::new(0),
        done: done_receiver,
    };
    let queue = Queue {
        receiver,
        queued,
        drained,
        ledger,
        done,
        #[cfg(test)]
        hold: None,
    };
    (outbox, queue)
}

fn lock(ledger: &Mutex<Ledger>) -> MutexGuard<'_, Ledger> {
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Outbox {
    /// Whether an inventory page or output may go in now.
    pub fn has_room_for_bulk(&self) -> bool {
        let frames = QUEUE_FRAMES - self.sender.capacity();
        frames < BULK_FRAMES && self.queued.load(Ordering::SeqCst) < BULK_BYTES
    }

    /// Whether a viewer's request may go in now: half the queue stays for
    /// heartbeats and handshakes, however many requests its clients make.
    pub fn has_room_for_requests(&self) -> bool {
        let frames = QUEUE_FRAMES - self.sender.capacity();
        frames < QUEUE_FRAMES / 2 && self.queued.load(Ordering::SeqCst) < QUEUE_BYTES / 2
    }

    /// Notified each time the writer takes a frame.
    pub fn drained(&self) -> Arc<Notify> {
        Arc::clone(&self.drained)
    }

    /// Queues a frame; `false` when the queue is full or the writer gone.
    /// `tag` names the session whose message it seals, so it can be
    /// discarded while it waits.
    pub fn push_for(&self, tag: Option<Tag>, text: String) -> bool {
        let size = text.len();
        if self.queued.fetch_add(size, Ordering::SeqCst) + size > QUEUE_BYTES {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            return false;
        }
        // Counted first: the writer may take it as soon as it is in.
        if let Some(tag) = &tag {
            *lock(&self.ledger).pending.entry(tag.clone()).or_default() += 1;
        }
        if let Err(error) = self.sender.try_send(Frame { text, tag }) {
            self.queued.fetch_sub(size, Ordering::SeqCst);
            let tag = match error {
                mpsc::error::TrySendError::Full(frame)
                | mpsc::error::TrySendError::Closed(frame) => frame.tag,
            };
            if let Some(tag) = tag {
                lock(&self.ledger).settle(&tag);
            }
            return false;
        }
        self.pushed.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Discards the messages of session `tag` still queued, and `true`
    /// when there were any: the session can no longer send, since its
    /// peer would miss the ones discarded.
    pub fn discard(&self, tag: &Tag) -> bool {
        let mut ledger = lock(&self.ledger);
        if ledger.pending.get(tag).is_some_and(|count| *count > 0) {
            ledger.discarded.insert(tag.clone());
            return true;
        }
        false
    }

    /// Resolves once the writer is done with every frame queued so far,
    /// written or discarded, or is gone.
    pub fn flushed(&self) -> impl std::future::Future<Output = ()> + std::marker::Send + 'static {
        let queued = self.pushed.load(Ordering::SeqCst);
        let mut done = self.done.clone();
        async move {
            let _ = done.wait_for(|done| *done >= queued).await;
        }
    }
}

impl Ledger {
    /// One message of `tag` left the queue. Returns whether it goes out.
    fn settle(&mut self, tag: &Tag) -> bool {
        let discarded = self.discarded.contains(tag);
        let left = self.pending.get_mut(tag).map(|count| {
            *count = count.saturating_sub(1);
            *count
        });
        if left.unwrap_or(0) == 0 {
            self.pending.remove(tag);
            self.discarded.remove(tag);
        }
        !discarded
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
    while let Some(Frame { text, tag }) = queue.receiver.recv().await {
        let size = text.len();
        queue.queued.fetch_sub(size, Ordering::SeqCst);
        queue.drained.notify_one();
        let wait = bucket.wait_for(size, Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        #[cfg(test)]
        queue.held().await;
        // Looked at last, right before the write: a session discarded while
        // the frame waited loses it.
        let goes = tag
            .as_ref()
            .is_none_or(|tag| lock(&queue.ledger).settle(tag));
        if goes {
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
        queue.done.send_modify(|done| *done += 1);
    }
    let _ = tokio::time::timeout(WRITE_TIMEOUT, sink.close()).await;
}

#[cfg(test)]
impl Queue {
    async fn held(&self) {
        while self
            .hold
            .as_ref()
            .is_some_and(|hold| hold.load(Ordering::SeqCst))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
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
        assert!(outbox.push_for(None, "ping".into()));
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
            assert!(outbox.push_for(None, "x".into()));
        }
        assert!(!outbox.has_room_for_bulk());
        // A heartbeat still goes in.
        assert!(outbox.push_for(None, "keep".into()));

        let (outbox, _queue) = queue();
        assert!(outbox.push_for(None, "x".repeat(BULK_BYTES)));
        assert!(!outbox.has_room_for_bulk());
    }

    #[test]
    fn the_queue_is_bounded_in_frames_and_bytes() {
        let (outbox, _queue) = queue();
        for _ in 0..QUEUE_FRAMES {
            assert!(outbox.push_for(None, "x".into()));
        }
        assert!(!outbox.push_for(None, "x".into()));

        let (outbox, _queue) = queue();
        assert!(outbox.push_for(None, "x".repeat(QUEUE_BYTES - 10)));
        assert!(!outbox.push_for(None, "x".repeat(11)));
        assert!(outbox.push_for(None, "x".repeat(10)));
    }

    /// A sink that takes every frame at once and keeps it.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl Sink<Message> for Recorder {
        type Error = std::io::Error;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.0
                .lock()
                .unwrap()
                .push(item.to_text().unwrap().to_string());
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// The messages of a discarded session never leave, the others do, and
    /// the barrier resolves only once the writer is past every frame queued
    /// before it.
    #[tokio::test]
    async fn a_discarded_session_loses_what_is_still_queued_behind_a_barrier() {
        let (outbox, mut queue) = queue();
        let hold = Arc::new(std::sync::atomic::AtomicBool::new(true));
        queue.hold = Some(Arc::clone(&hold));
        let sink = Recorder::default();
        let writer = tokio::spawn(run(sink.clone(), queue, Bucket::paced(Instant::now())));
        let (cut, kept): (Tag, Tag) = (
            ("mA".into(), SessionId::random().unwrap()),
            ("mC".into(), SessionId::random().unwrap()),
        );
        assert!(outbox.push_for(Some(cut.clone()), "a1".into()));
        assert!(outbox.push_for(Some(kept.clone()), "c1".into()));
        assert!(outbox.push_for(Some(cut.clone()), "a2".into()));
        assert!(outbox.push_for(None, "close".into()));

        assert!(outbox.discard(&cut));
        assert!(!outbox.discard(&("mD".into(), SessionId::random().unwrap())));
        let mut flushed = Box::pin(outbox.flushed());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut flushed)
                .await
                .is_err(),
            "the writer still holds them"
        );
        hold.store(false, Ordering::SeqCst);
        flushed.await;

        assert_eq!(*sink.0.lock().unwrap(), ["c1", "close"]);
        // The session's next messages are its own again.
        assert!(outbox.push_for(Some(cut), "a3".into()));
        outbox.flushed().await;
        assert_eq!(sink.0.lock().unwrap().last().unwrap(), "a3");
        drop(outbox);
        writer.await.unwrap();
    }
}
