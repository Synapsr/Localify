//! One send queue per relay socket, paced below the relay's budget
//! (`docs/remote-machines.md`, "Limits", "Send pacing"): at most 40 frames
//! and 384 KiB a second for all sessions together, whatever role this
//! machine plays in each. Today every frame is a handshake, a heartbeat, a
//! pairing message or a ping, so one first-in first-out queue is enough;
//! inventory pages and output, which go after those, arrive with PR 3.

use std::time::Duration;

use futures_util::{Sink, SinkExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tracing::debug;

pub const FRAMES_PER_SECOND: f64 = 40.0;
pub const BYTES_PER_SECOND: f64 = 384.0 * 1024.0;

/// A token bucket for frames and bytes. It holds a second's worth, so a
/// burst never comes near the relay's 200 frames.
#[derive(Debug, Clone)]
pub struct Bucket {
    frames: f64,
    bytes: f64,
    frame_rate: f64,
    byte_rate: f64,
    updated: Instant,
}

impl Bucket {
    pub fn new(frame_rate: f64, byte_rate: f64, now: Instant) -> Self {
        Self {
            frames: frame_rate,
            bytes: byte_rate,
            frame_rate,
            byte_rate,
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.updated = now;
        self.frames = (self.frames + elapsed * self.frame_rate).min(self.frame_rate);
        self.bytes = (self.bytes + elapsed * self.byte_rate).min(self.byte_rate);
    }

    /// How long to wait before a frame of `size` bytes may go. A frame
    /// larger than a second's worth of bytes waits until the bucket is full.
    pub fn wait_for(&mut self, size: usize, now: Instant) -> Duration {
        self.refill(now);
        let size = (size as f64).min(self.byte_rate);
        let frames = ((1.0 - self.frames) / self.frame_rate).max(0.0);
        let bytes = ((size - self.bytes) / self.byte_rate).max(0.0);
        Duration::from_secs_f64(frames.max(bytes))
    }

    /// Takes a frame's tokens; call once [`Bucket::wait_for`] is zero.
    pub fn take(&mut self, size: usize, now: Instant) {
        self.refill(now);
        self.frames -= 1.0;
        self.bytes -= (size as f64).min(self.byte_rate);
    }
}

/// Writes every queued text frame to the socket, paced. Returns once the
/// queue is closed, or the socket fails.
pub async fn run<S>(mut sink: S, mut queue: mpsc::UnboundedReceiver<String>, mut bucket: Bucket)
where
    S: Sink<Message> + Unpin,
{
    while let Some(text) = queue.recv().await {
        let size = text.len();
        let wait = bucket.wait_for(size, Instant::now());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        bucket.take(size, Instant::now());
        if sink.send(Message::text(text)).await.is_err() {
            debug!("The relay socket closed while sending");
            return;
        }
    }
    let _ = sink.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_goes_at_once_then_frames_wait_their_turn() {
        let start = Instant::now();
        let mut bucket = Bucket::new(FRAMES_PER_SECOND, BYTES_PER_SECOND, start);
        for _ in 0..40 {
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
        let mut bucket = Bucket::new(FRAMES_PER_SECOND, BYTES_PER_SECOND, start);
        let (mut frames, mut bytes) = (0_u32, 0_usize);
        while now < start + Duration::from_secs(10) {
            now += bucket.wait_for(2048, now);
            bucket.take(2048, now);
            frames += 1;
            bytes += 2048;
        }
        // The relay allows 50 frames and 512 KiB a second over 10 s.
        assert!(frames <= 10 * 40 + 40, "{frames}");
        assert!(bytes <= 11 * 384 * 1024, "{bytes}");
        assert!(frames < 500 && bytes < 10 * 512 * 1024);
    }

    #[test]
    fn large_frames_are_paced_by_bytes() {
        let start = Instant::now();
        let mut bucket = Bucket::new(FRAMES_PER_SECOND, BYTES_PER_SECOND, start);
        bucket.take(60 * 1024, start);
        let mut now = start;
        let mut sent = 1;
        while now < start + Duration::from_secs(1) {
            now += bucket.wait_for(60 * 1024, now);
            bucket.take(60 * 1024, now);
            sent += 1;
        }
        // 384 KiB a second plus the bucket's first second: 13 frames of 60 KiB.
        assert!(sent <= 14, "{sent}");
    }
}
