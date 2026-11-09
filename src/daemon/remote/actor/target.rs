//! The target's side of a session (`docs/remote-machines.md`, "Inventory"
//! and "Output"): snapshots and output for a viewer that asked, while it
//! keeps asking, and only while `share` is on. Nothing is computed or sent
//! for a viewer that asked for nothing.
//!
//! Inventory pages and output wait here and go out only while the send
//! queue has room (`pacer`), pages first, then output taken in turn between
//! follows, each one within the window its viewer acknowledged.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use super::{Actor, Command, End, Failure, Key};
use crate::application::inventory::{Snapshot, Switches, pages};
use crate::daemon::remote::local::RunLookup;
use crate::infrastructure::remote::store::DEFAULT_SETTINGS;
use crate::infrastructure::remote::wire::{
    Have, MAX_MESSAGE_BYTES, MAX_READ_BYTES, MachineSummary, OutputData, OutputLine, Purpose, Role,
    Rotated, SessionMessage,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::output;

/// At most this many follows per session, and per target.
const FOLLOWS_PER_SESSION: usize = 2;
const FOLLOWS_PER_TARGET: usize = 4;
/// `inventory.get`s and `output.read`s in flight, from their admission until
/// their last answer is in the send queue: per session, and per target.
const GETS_PER_SESSION: usize = 4;
const GETS_PER_TARGET: usize = 16;
const READS_PER_SESSION: usize = 8;
const READS_PER_TARGET: usize = 32;
/// Answers waiting for room in the send queue, in bytes: per session, and
/// per target. A whole snapshot (32 pages of 32 KiB) fits in one session's.
const PENDING_PER_SESSION: usize = 1024 * 1024 + 64 * 1024;
const PENDING_PER_TARGET: usize = 4 * 1024 * 1024;

const NOT_SHARED: &str =
    "This machine doesn't share its projects and output. Turn on sharing in Localify on it.";
const FULL: &str = "This machine has too many answers waiting to be sent. Try again in a moment.";

/// What one viewer asked this machine for.
#[derive(Default)]
pub(super) struct Served {
    watch: Option<Watch>,
    follows: Vec<Follow>,
    /// `inventory.get`s waiting for the snapshot being built.
    gets: Vec<(String, Option<Have>)>,
    /// Pages and reads waiting for room in the send queue.
    bulk: VecDeque<Pending>,
    /// What `bulk` holds, in bytes.
    pending_bytes: usize,
    /// Gets and reads admitted and not fully queued for sending yet.
    in_flight: Vec<(String, Kind)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Get,
    Read,
}

/// An answer waiting for room in the send queue.
struct Pending {
    message: SessionMessage,
    bytes: usize,
    re: String,
    /// The request's last answer: it is no longer in flight once this is
    /// queued.
    last: bool,
}

struct Watch {
    id: String,
    /// The revision the viewer holds.
    have: Option<Have>,
    /// Whether it heard anything yet: a watch that starts with the current
    /// revision is told so once.
    answered: bool,
    renewed: Instant,
}

struct Follow {
    id: String,
    run_id: String,
    dir: PathBuf,
    /// Sent up to here.
    next: u64,
    /// Acknowledged up to here.
    acked: u64,
    window: u64,
    renewed: Instant,
    /// The first `output.data` goes at once, even empty.
    fresh: bool,
    /// Worth a look: new output may be there.
    ready: bool,
}

impl Follow {
    fn unacked(&self) -> u64 {
        self.next.saturating_sub(self.acked)
    }
}

impl Served {
    /// Whether the viewer watches the inventory.
    pub(super) fn watches(&self) -> bool {
        self.watch.is_some()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.watch.is_none()
            && self.follows.is_empty()
            && self.gets.is_empty()
            && self.bulk.is_empty()
            && self.in_flight.is_empty()
    }

    /// A watch or a follow nothing renewed in `after` ends.
    pub(super) fn expire(&mut self, now: Instant, after: Duration) {
        if self
            .watch
            .as_ref()
            .is_some_and(|watch| now.duration_since(watch.renewed) >= after)
        {
            self.watch = None;
        }
        self.follows
            .retain(|follow| now.duration_since(follow.renewed) < after);
    }

    fn renew(&mut self, ids: &[String], now: Instant) {
        if let Some(watch) = self.watch.as_mut().filter(|watch| ids.contains(&watch.id)) {
            watch.renewed = now;
        }
        for follow in &mut self.follows {
            if ids.contains(&follow.id) {
                follow.renewed = now;
            }
        }
    }

    /// Ends everything, waiting answers included, and returns the ids that
    /// were open.
    fn clear(&mut self) -> Vec<String> {
        let mut ids: Vec<String> = self.in_flight.drain(..).map(|(id, _)| id).collect();
        ids.extend(self.watch.take().map(|watch| watch.id));
        ids.extend(self.follows.drain(..).map(|follow| follow.id));
        self.gets.clear();
        self.bulk.clear();
        self.pending_bytes = 0;
        ids
    }

    fn count(&self, kind: Kind) -> usize {
        self.in_flight.iter().filter(|(_, k)| *k == kind).count()
    }

    fn done(&mut self, re: &str) {
        self.in_flight.retain(|(id, _)| id != re);
    }

    /// Whether `id` names a request still going: waiting, watched,
    /// followed, or with answers still to send.
    pub(super) fn is_active(&self, id: &str) -> bool {
        self.in_flight.iter().any(|(active, _)| active == id)
            || self.gets.iter().any(|(active, _)| active == id)
            || self.watch.as_ref().is_some_and(|watch| watch.id == id)
            || self.follows.iter().any(|follow| follow.id == id)
            || self.bulk.iter().any(|pending| pending.re == id)
    }

    /// Drops the answers still waiting for `re`; a request whose last
    /// answer goes with them is no longer in flight.
    fn drop_pending(&mut self, re: &str) {
        let mut freed = 0;
        let mut finished = false;
        self.bulk.retain(|pending| {
            let kept = pending.re != re;
            if !kept {
                freed += pending.bytes;
                finished |= pending.last;
            }
            kept
        });
        self.pending_bytes -= freed;
        if finished {
            self.done(re);
        }
    }

    fn push(&mut self, re: &str, message: SessionMessage, last: bool) {
        let bytes = encoded_size(&message);
        self.pending_bytes += bytes;
        self.bulk.push_back(Pending {
            message,
            bytes,
            re: re.to_string(),
            last,
        });
    }

    fn pop(&mut self) -> Option<SessionMessage> {
        let pending = self.bulk.pop_front()?;
        self.pending_bytes -= pending.bytes;
        if pending.last {
            self.done(&pending.re);
        }
        Some(pending.message)
    }
}

/// The target's state across sessions.
pub(super) struct Shelf {
    /// `share`, as this task last learned it: from a settings change, which
    /// waits for this task to act on it before it answers, and from the file
    /// each second, for a change made some other way.
    pub(super) sharing: bool,
    /// A snapshot is being built: one at a time.
    building: bool,
    /// While watched, the next look for changes.
    next_look: Option<Instant>,
    /// Whose turn it is among the follows.
    turn: usize,
    /// When to read the switches from the file again.
    next_recheck: Option<Instant>,
}

impl Shelf {
    pub(super) fn new(sharing: bool) -> Self {
        Self {
            sharing,
            building: false,
            next_look: None,
            turn: 0,
            next_recheck: None,
        }
    }
}

impl Actor {
    fn sharing(&self) -> bool {
        self.shelf.sharing
    }

    /// Reads the settings file, which every change is written to
    /// atomically before anyone is told, and acts on it: `share` when it
    /// changed, and an access request whose generation is no longer the
    /// file's. Never from a notice: one can arrive after a newer change.
    pub(super) fn reload_settings(&mut self) {
        let file = self.shared.store.settings_file().ok();
        let share = file.is_some_and(|file| file.settings.share);
        if share != self.shelf.sharing {
            self.on_share(share);
        }
        self.recheck_access(file.map(|file| file.generation));
    }

    /// `share` changed here: turned off, every watch, follow and waiting
    /// answer ends now, before the change is confirmed, so nothing read
    /// after it is sent.
    fn on_share(&mut self, share: bool) {
        self.shelf.sharing = share;
        if !share {
            self.cut_queued(NOT_SHARED);
            self.end_served("not_shared", NOT_SHARED);
            self.drop_control_answers(NOT_SHARED);
        }
    }

    /// The answers waiting in every target session, in bytes, and the
    /// requests of `kind` in flight.
    fn totals(&self, kind: Kind) -> (usize, usize) {
        self.sessions
            .values()
            .fold((0, 0), |(bytes, count), session| {
                (
                    bytes + session.served.pending_bytes,
                    count + session.served.count(kind),
                )
            })
    }

    /// Whether `bytes` more answers fit, in the session and in all of them.
    fn fits(&self, key: &Key, bytes: usize) -> bool {
        let session = self
            .sessions
            .get(key)
            .map_or(0, |session| session.served.pending_bytes);
        let (total, _) = self.totals(Kind::Get);
        session + bytes <= PENDING_PER_SESSION && total + bytes <= PENDING_PER_TARGET
    }

    /// Admits a get or a read before any work is done for it, or refuses
    /// it: `too_many` past the count, `busy` past the bytes.
    fn admit(&mut self, key: &Key, id: &str, kind: Kind, bytes: usize) -> bool {
        let (per_session, per_target) = match kind {
            Kind::Get => (GETS_PER_SESSION, GETS_PER_TARGET),
            Kind::Read => (READS_PER_SESSION, READS_PER_TARGET),
        };
        let mine = self
            .sessions
            .get(key)
            .map_or(0, |session| session.served.count(kind));
        let (_, all) = self.totals(kind);
        if mine >= per_session || all >= per_target {
            self.refuse(key, id, "too_many", "Too many requests at once.");
            return false;
        }
        if !self.fits(key, bytes) {
            self.answer(key, id, busy(id));
            return false;
        }
        match self.served(key) {
            Some(served) => {
                served.in_flight.push((id.to_string(), kind));
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    fn note_pending(&self) {
        let (total, _) = self.totals(Kind::Get);
        self.shared
            .pending_peak
            .fetch_max(total, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(not(test))]
    fn note_pending(&self) {}

    fn served_sessions(&self) -> Vec<Key> {
        self.sessions
            .iter()
            .filter(|(_, session)| {
                session.role == Role::Target
                    && session.purpose == Purpose::Session
                    && !session.served.is_empty()
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn served(&mut self, key: &Key) -> Option<&mut Served> {
        self.sessions
            .get_mut(key)
            .map(|session| &mut session.served)
    }

    fn refuse(&mut self, key: &Key, re: &str, code: &str, message: impl Into<String>) {
        self.answer(key, re, SessionMessage::refusal(re, code, message));
    }

    /// A short answer (`unchanged`, an error) queued behind the others, so
    /// a viewer flooding requests waits its turn rather than filling the
    /// send queue. Past the session's bytes, the session ends.
    fn answer(&mut self, key: &Key, re: &str, message: SessionMessage) {
        if !self.fits(key, encoded_size(&message)) {
            return self.end(
                key,
                End::Close,
                &Failure::new("too_many", "The viewer sent more requests than it reads."),
            );
        }
        if let Some(served) = self.served(key) {
            served.push(re, message, false);
        }
    }

    /// A viewer's request: inventory or output.
    pub(super) fn on_request(&mut self, key: &Key, request: SessionMessage) {
        let now = Instant::now();
        match request {
            SessionMessage::InventoryUnwatch { id } => {
                if let Some(served) = self.served(key)
                    && served.watch.as_ref().is_some_and(|watch| watch.id == id)
                {
                    served.watch = None;
                }
            }
            SessionMessage::OutputAck { re, offset } => {
                let follow = self
                    .served(key)
                    .and_then(|served| served.follows.iter_mut().find(|follow| follow.id == re));
                if let Some(follow) = follow {
                    follow.acked = offset.clamp(follow.acked, follow.next);
                    follow.renewed = now;
                    follow.ready = true;
                }
            }
            SessionMessage::OutputUnfollow { re } => {
                if let Some(served) = self.served(key) {
                    served.follows.retain(|follow| follow.id != re);
                }
            }
            request => self.on_share_request(key, request),
        }
    }

    /// A request for what `share` covers.
    fn on_share_request(&mut self, key: &Key, request: SessionMessage) {
        let id = match &request {
            SessionMessage::InventoryGet { id, .. }
            | SessionMessage::InventoryWatch { id, .. }
            | SessionMessage::OutputRead { id, .. }
            | SessionMessage::OutputFollow { id, .. } => id.clone(),
            _ => return,
        };
        // An id names one request at a time, whatever its kind: a second
        // one would share its answers and its slot.
        let in_use = self.sessions.get(key).is_some_and(|session| {
            session.served.is_active(&id)
                || session
                    .access
                    .as_ref()
                    .is_some_and(|waiting| waiting.re == id)
        });
        if in_use {
            return self.refuse(
                key,
                &id,
                "bad_request",
                "This request id is already in use.",
            );
        }
        if !self.sharing() {
            return self.refuse(key, &id, "not_shared", NOT_SHARED);
        }
        match request {
            SessionMessage::InventoryGet { id, have } => self.get_inventory(key, id, have),
            SessionMessage::InventoryWatch { id, have } => self.watch_inventory(key, id, have),
            SessionMessage::OutputRead {
                id,
                run_id,
                offset,
                max_bytes,
            } => self.read_output(key, &id, &run_id, offset, max_bytes),
            SessionMessage::OutputFollow {
                id,
                run_id,
                offset,
                window,
                tail,
            } => self.follow_output(key, id, run_id, offset, window, tail),
            _ => {}
        }
    }

    // --- Inventory -----------------------------------------------------------

    fn get_inventory(&mut self, key: &Key, id: String, have: Option<Have>) {
        // Its pages are counted once the snapshot is built.
        if !self.admit(key, &id, Kind::Get, 0) {
            return;
        }
        if let Some(served) = self.served(key) {
            served.gets.push((id, have));
        }
        self.build_snapshot();
    }

    fn watch_inventory(&mut self, key: &Key, id: String, have: Option<Have>) {
        let Some(served) = self.served(key) else {
            return;
        };
        if served.watch.as_ref().is_some_and(|watch| watch.id != id) {
            return self.refuse(key, &id, "too_many", "This session already watches.");
        }
        served.watch = Some(Watch {
            id,
            have,
            answered: false,
            renewed: Instant::now(),
        });
        self.build_snapshot();
    }

    /// Builds a snapshot off this task; one at a time.
    pub(super) fn build_snapshot(&mut self) {
        if self.shelf.building {
            return;
        }
        self.shelf.building = true;
        let source = Arc::clone(&self.ctx.source);
        let commands = self.commands.clone();
        let switches = Switches {
            settings: self
                .shared
                .store
                .settings()
                .unwrap_or(DEFAULT_SETTINGS)
                .into(),
            app: source.app().into(),
        };
        self.spawn(async move {
            let result = source
                .snapshot(switches)
                .await
                .map(Arc::new)
                .map_err(|error| format!("{error:#}"));
            let _ = commands.send(Command::Snapshot { result });
        });
    }

    /// A snapshot is built: answers every waiting `get`, and sends each
    /// watch what changed.
    pub(super) fn on_snapshot(&mut self, result: Result<Arc<Snapshot>, String>) {
        self.shelf.building = false;
        self.shelf.next_look = Some(Instant::now() + self.shared.timings.watch_every);
        if !self.sharing() {
            return self.end_served("not_shared", NOT_SHARED);
        }
        let summary = self.summary();
        // The revision moves with the summary as much as with the items.
        let numbered = result.map(|snapshot| {
            let current = self
                .shared
                .sent_revisions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .number(&snapshot, &summary);
            (snapshot, current)
        });
        for key in self.served_sessions() {
            match &numbered {
                Ok((snapshot, current)) => {
                    self.answer_snapshot(&key, snapshot, &summary, current);
                }
                Err(message) => {
                    let gets: Vec<String> = self
                        .served(&key)
                        .map(|served| served.gets.drain(..).map(|(id, _)| id).collect())
                        .unwrap_or_default();
                    for id in gets {
                        if let Some(served) = self.served(&key) {
                            served.done(&id);
                        }
                        self.refuse(&key, &id, "unavailable", message.clone());
                    }
                }
            }
        }
    }

    /// Answers `key`'s gets and watch with `snapshot`, which goes out with
    /// `summary` as revision `current`.
    fn answer_snapshot(
        &mut self,
        key: &Key,
        snapshot: &Snapshot,
        summary: &MachineSummary,
        current: &Have,
    ) {
        let Some(served) = self.served(key) else {
            return;
        };
        let gets = std::mem::take(&mut served.gets);
        for (id, have) in gets {
            if have.as_ref() == Some(current) {
                if let Some(served) = self.served(key) {
                    served.done(&id);
                }
                self.answer(key, &id, unchanged(&id, current));
            } else {
                self.queue_pages(key, &id, (snapshot, current), summary, true);
            }
        }
        let Some(watch) = self.served(key).and_then(|served| served.watch.as_mut()) else {
            return;
        };
        let id = watch.id.clone();
        if watch.have.as_ref() == Some(current) {
            let first = !watch.answered;
            watch.answered = true;
            if first {
                self.answer(key, &id, unchanged(&id, current));
            }
        } else if self.queue_pages(key, &id, (snapshot, current), summary, false)
            && let Some(watch) = self.served(key).and_then(|served| served.watch.as_mut())
        {
            watch.have = Some(current.clone());
            watch.answered = true;
        }
    }

    /// Queues the snapshot's pages answering `re`, in place of older ones
    /// still waiting, when they fit. A get that does not fit is refused
    /// `busy` and is over; a watch tries again at its next look. Returns
    /// whether they were queued.
    fn queue_pages(
        &mut self,
        key: &Key,
        re: &str,
        snapshot: (&Snapshot, &Have),
        summary: &MachineSummary,
        get: bool,
    ) -> bool {
        // Pages of an older revision still waiting are dropped.
        if let Some(served) = self.served(key) {
            served.drop_pending(re);
        }
        let pages = match pages_of(re, snapshot, summary) {
            Ok(pages) => pages,
            Err(error) => {
                if let Some(served) = self.served(key) {
                    served.done(re);
                }
                self.refuse(key, re, "unavailable", error);
                return false;
            }
        };
        let bytes = pages.iter().map(encoded_size).sum();
        if !self.fits(key, bytes) {
            if get {
                if let Some(served) = self.served(key) {
                    served.done(re);
                }
                self.answer(key, re, busy(re));
            }
            return false;
        }
        let Some(served) = self.served(key) else {
            return false;
        };
        let count = pages.len();
        for (index, page) in pages.into_iter().enumerate() {
            served.push(re, page, get && index + 1 == count);
        }
        self.note_pending();
        true
    }

    fn summary(&self) -> MachineSummary {
        MachineSummary {
            machine_id: self.ctx.self_id.clone(),
            name: self.ctx.name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            os: if cfg!(target_os = "macos") {
                "macos"
            } else {
                "linux"
            }
            .to_string(),
            settings: self.shared.store.settings().unwrap_or(DEFAULT_SETTINGS),
            app: self.ctx.source.app(),
        }
    }

    // --- Output --------------------------------------------------------------

    /// The output folder of a service's last run, or the refusal.
    fn locate(&self, run_id: &str) -> Result<PathBuf, (&'static str, String)> {
        let Ok(run_id) = RunId::parse(run_id) else {
            return Err(("bad_request", format!("'{run_id}' is not a run id.")));
        };
        match self.ctx.source.run_output(&run_id) {
            Ok(Ok(dir)) => Ok(dir),
            Ok(Err(RunLookup::Gone)) => Err(gone()),
            Ok(Err(RunLookup::Unavailable(reason))) => Err((
                "output_unavailable",
                format!("This run's output can't be read from another machine ({reason})."),
            )),
            Err(error) => Err(("output_unavailable", format!("{error:#}"))),
        }
    }

    /// Admitted with room for the largest answer before anything is read;
    /// in flight until its answer is in the send queue.
    fn read_output(&mut self, key: &Key, id: &str, run_id: &str, offset: u64, max_bytes: u32) {
        if !self.admit(key, id, Kind::Read, MAX_MESSAGE_BYTES) {
            return;
        }
        let read = self
            .locate(run_id)
            .and_then(|dir| read_fitting(&dir, id, run_id, offset, max_bytes as usize));
        let Some(served) = self.served(key) else {
            return;
        };
        match read {
            Ok(data) => {
                served.push(id, SessionMessage::OutputData(data), true);
                self.note_pending();
            }
            Err((code, message)) => {
                served.done(id);
                self.refuse(key, id, code, message);
            }
        }
    }

    fn follow_output(
        &mut self,
        key: &Key,
        id: String,
        run_id: String,
        offset: u64,
        window: u32,
        tail: Option<u64>,
    ) {
        let total: usize = self
            .sessions
            .values()
            .map(|session| session.served.follows.len())
            .sum();
        let Some(served) = self.served(key) else {
            return;
        };
        if served.follows.len() >= FOLLOWS_PER_SESSION || total >= FOLLOWS_PER_TARGET {
            return self.refuse(key, &id, "too_many", "Too many follows at once.");
        }
        let dir = match self.locate(&run_id) {
            Ok(dir) => dir,
            Err((code, message)) => return self.refuse(key, &id, code, message),
        };
        let start = match tail {
            None => offset,
            Some(tail) => match output::extent(&dir) {
                Ok((first, total)) => first.max(total.saturating_sub(tail)),
                Err(error) => {
                    return self.refuse(key, &id, "output_unavailable", format!("{error:#}"));
                }
            },
        };
        let follow = Follow {
            id,
            run_id,
            dir,
            next: start,
            acked: start,
            window: u64::from(window),
            renewed: Instant::now(),
            fresh: true,
            ready: true,
        };
        if let Some(served) = self.served(key) {
            served.follows.push(follow);
        }
    }

    /// Every follow is worth a look again.
    pub(super) fn on_poll(&mut self) {
        for session in self.sessions.values_mut() {
            for follow in &mut session.served.follows {
                follow.ready = true;
            }
        }
    }

    /// Fills the send queue with what waits, while it has room.
    pub(super) fn pump(&mut self) {
        self.pump_taps();
        while self.failed.is_none() && self.out.has_room_for_bulk() {
            if let Some((key, message)) = self.next_bulk() {
                // Nothing that `share` covers goes out once it is off;
                // refusals do.
                let shared = !matches!(message, SessionMessage::Error { .. });
                if self.sharing() || !shared {
                    self.send_message(&key, &message);
                }
                continue;
            }
            if !self.sharing() || !self.send_output() {
                break;
            }
        }
    }

    fn next_bulk(&mut self) -> Option<(Key, SessionMessage)> {
        let key = self
            .sessions
            .iter()
            .find(|(_, session)| !session.served.bulk.is_empty())
            .map(|(key, _)| key.clone())?;
        let message = self.served(&key)?.pop()?;
        Some((key, message))
    }

    /// Reads and sends for one follow, taken in turn. `false` when no
    /// follow has anything to look at.
    fn send_output(&mut self) -> bool {
        let mut ready: Vec<(Key, String)> = self
            .sessions
            .iter()
            .flat_map(|(key, session)| {
                session
                    .served
                    .follows
                    .iter()
                    .filter(|follow| follow.ready && follow.unacked() < follow.window)
                    .map(move |follow| (key.clone(), follow.id.clone()))
            })
            .collect();
        if ready.is_empty() {
            return false;
        }
        ready.sort_by(|left, right| {
            (&left.0.0, left.0.1.bytes(), &left.1).cmp(&(&right.0.0, right.0.1.bytes(), &right.1))
        });
        let (key, id) = ready.swap_remove(self.shelf.turn % ready.len());
        self.shelf.turn = self.shelf.turn.wrapping_add(1);
        self.send_follow(&key, &id);
        true
    }

    fn send_follow(&mut self, key: &Key, id: &str) {
        // Checked again right before reading: nothing is read once `share`
        // is off.
        if !self.sharing() {
            return;
        }
        let Some(served) = self.served(key) else {
            return;
        };
        let Some(index) = served.follows.iter().position(|follow| follow.id == id) else {
            return;
        };
        let follow = &mut served.follows[index];
        let room = (follow.window - follow.unacked()).min(u64::from(MAX_READ_BYTES));
        let room = usize::try_from(room).unwrap_or(0);
        let data = match read_fitting(&follow.dir, id, &follow.run_id, follow.next, room) {
            Ok(data) => data,
            Err((code, message)) => {
                served.follows.remove(index);
                return self.refuse(key, id, code, message);
            }
        };
        let progressed = follow.fresh
            || data.next != follow.next
            || !data.lines.is_empty()
            || data.ended
            || data.rotated.is_some();
        if !progressed {
            follow.ready = false;
            return;
        }
        follow.fresh = false;
        follow.acked = follow.acked.max(data.offset);
        follow.next = data.next;
        if data.ended {
            served.follows.remove(index);
        }
        if self.sharing() {
            self.send_message(key, &SessionMessage::OutputData(data));
        }
    }

    // --- Lifetimes -----------------------------------------------------------

    /// A `keep` names the watches and follows its viewer still shows.
    pub(super) fn renew(&mut self, key: &Key, ids: &[String]) {
        if let Some(served) = self.served(key) {
            served.renew(ids, Instant::now());
        }
    }

    /// Each second: a look for changes while watched, and the end of what
    /// `share` no longer allows.
    pub(super) fn serve_tick(&mut self, now: Instant) {
        // A change made without the management socket.
        if self.shelf.next_recheck.is_none_or(|at| now >= at) {
            self.shelf.next_recheck = Some(now + self.shared.timings.settings_recheck);
            self.reload_settings();
        }
        let served = self.served_sessions();
        if served.is_empty() || !self.sharing() {
            return;
        }
        let watched = served.iter().any(|key| {
            self.sessions
                .get(key)
                .is_some_and(|session| session.served.watch.is_some())
        });
        let due = self.shelf.next_look.is_none_or(|at| now >= at);
        if watched && due {
            self.build_snapshot();
        }
    }

    /// `share` was turned off: a viewer's session with messages still in
    /// the send queue has them discarded, and ends with a relay `close`,
    /// since its viewer could read nothing sealed after them. What was
    /// already written is the viewer's; nothing queued before the change
    /// leaves once it is confirmed.
    fn cut_queued(&mut self, message: &str) {
        let viewers: Vec<Key> = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session.role == Role::Target && session.purpose == Purpose::Session
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in viewers {
            if self.out.discard(&key) {
                self.end(&key, End::Close, &Failure::new("not_shared", message));
            }
        }
    }

    /// Ends every watch, follow and waiting request, with `code`.
    fn end_served(&mut self, code: &str, message: &str) {
        for key in self.served_sessions() {
            let ids = self.served(&key).map(Served::clear).unwrap_or_default();
            for id in ids {
                self.refuse(&key, &id, code, message);
            }
        }
    }
}

fn unchanged(re: &str, current: &Have) -> SessionMessage {
    SessionMessage::InventoryUnchanged {
        re: re.to_string(),
        epoch: current.epoch.clone(),
        rev: current.rev,
    }
}

fn busy(re: &str) -> SessionMessage {
    SessionMessage::Error {
        re: Some(re.to_string()),
        code: "busy".into(),
        message: FULL.into(),
        retry_after: Some(1),
    }
}

/// What a message takes in memory, near enough: its JSON.
fn encoded_size(message: &SessionMessage) -> usize {
    serde_json::to_vec(message).map_or(MAX_MESSAGE_BYTES, |json| json.len())
}

fn gone() -> (&'static str, String) {
    (
        "gone",
        "This run is no longer the service's last one.".to_string(),
    )
}

/// The snapshot as pages of revision `current` answering `re`, each one
/// within 32 KiB.
fn pages_of(
    re: &str,
    (snapshot, current): (&Snapshot, &Have),
    summary: &MachineSummary,
) -> Result<Vec<SessionMessage>, String> {
    let split = pages(&snapshot.items).map_err(|error| format!("{error:#}"))?;
    let count = u32::try_from(split.len()).unwrap_or(u32::MAX);
    split
        .into_iter()
        .enumerate()
        .map(|(index, items)| {
            let items = items
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            Ok(SessionMessage::InventoryPage {
                re: re.to_string(),
                epoch: current.epoch.clone(),
                rev: current.rev,
                page: u32::try_from(index + 1).unwrap_or(u32::MAX),
                pages: count,
                machine: summary.clone(),
                items,
                truncated: snapshot.truncated,
            })
        })
        .collect()
}

/// One bounded read, as `output.data`, trimmed until the message fits in
/// 32 KiB: escaping and short lines can make the lines' JSON larger than
/// the bytes they came from.
fn read_fitting(
    dir: &Path,
    re: &str,
    run_id: &str,
    offset: u64,
    max_bytes: usize,
) -> Result<OutputData, (&'static str, String)> {
    // The next run of the service deleted it.
    if !dir.is_dir() {
        return Err(gone());
    }
    let mut max_bytes = max_bytes.max(1);
    loop {
        let read = output::read(dir, offset, max_bytes).map_err(|error| {
            if dir.is_dir() {
                ("bad_request", format!("{error:#}"))
            } else {
                gone()
            }
        })?;
        let data = OutputData {
            re: re.to_string(),
            run_id: run_id.to_string(),
            offset: read.offset,
            next: read.next,
            lines: read
                .lines
                .into_iter()
                .map(|line| OutputLine {
                    text: line.text,
                    cut: line.cut,
                })
                .collect(),
            rotated: read.rotated.map(|first_offset| Rotated { first_offset }),
            ended: read.ended,
            interrupted: read.interrupted,
        };
        let size = serde_json::to_vec(&SessionMessage::OutputData(data.clone()))
            .map(|json| json.len())
            .unwrap_or(usize::MAX);
        if size <= MAX_MESSAGE_BYTES || max_bytes <= 64 {
            return Ok(data);
        }
        max_bytes /= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::runs::output::{StreamWriter, create_run_dir};

    fn stream(bytes: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        create_run_dir(dir.path()).unwrap();
        StreamWriter::create(dir.path())
            .unwrap()
            .append(bytes)
            .unwrap();
        dir
    }

    fn size(data: &OutputData) -> usize {
        serde_json::to_vec(&SessionMessage::OutputData(data.clone()))
            .unwrap()
            .len()
    }

    /// Quotes double when escaped, and an empty line costs a dozen bytes of
    /// JSON: a full read of either would not fit.
    #[test]
    fn a_read_is_trimmed_until_its_message_fits() {
        for bytes in [
            vec![b'\n'; 20_000],
            [b"\"".repeat(16_000), vec![b'\n']].concat(),
        ] {
            let dir = stream(&bytes);
            let data = read_fitting(dir.path(), "r1", "run", 0, 16 * 1024).unwrap();

            assert!(size(&data) <= MAX_MESSAGE_BYTES, "{}", size(&data));
            assert!(data.next > 0, "it makes progress");
        }
    }

    #[test]
    fn a_read_that_fits_is_sent_whole() {
        let dir = stream(b"one\ntwo\n");
        let data = read_fitting(dir.path(), "r1", "run", 0, 16 * 1024).unwrap();

        assert_eq!((data.offset, data.next), (0, 8));
        assert_eq!(data.lines.len(), 2);
    }

    #[test]
    fn a_folder_that_went_away_is_gone() {
        let dir = stream(b"one\n");
        let path = dir.path().to_path_buf();
        drop(dir);

        let (code, _) = read_fitting(&path, "r1", "run", 0, 1024).unwrap_err();
        assert_eq!(code, "gone");
    }
}
