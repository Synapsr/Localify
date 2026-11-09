//! A Localify Cloud in the test process: the routes the link calls, and a
//! relay that routes frames as `docs/remote-machines.md`, "Relay", says
//! (`from` set from the socket, `peer_offline` for a machine with no
//! socket, `welcome` and `presence`), with hooks that drop, repeat, hold or
//! rewrite frames the way a misbehaving relay could.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::local::{LocalSource, RunLookup, SnapshotFuture, shell_failure};
use crate::application::inventory::{
    Context, InventoryItem, Revisions, RunFacts, RunOrigin, RunProcess, ServiceFacts, Switches,
    inventory_item,
};
use crate::infrastructure::app_bridge::{BridgeError, Handoff, LaunchRequest};
use crate::infrastructure::remote::control::{
    ControlRunner, ControlTimings, DetachedLaunch, Expected, Host, Live, Look, Order, Spawned,
};
use crate::infrastructure::remote::noise::{Handshake, SessionContext, SessionId, StaticKeys};
use crate::infrastructure::remote::requests::{Report, RequestEntries};
use crate::infrastructure::remote::wire::ControlPhase;
use crate::infrastructure::remote::wire::{AppPresence, FrameKind, Purpose, Send};
use crate::infrastructure::running_services::ServiceKey;
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::launches::{LaunchReport, RefusalCode, unix_ms_now};
use localify::desktop::shell_environment::ShellEnvironment;

/// What the relay does with one frame.
pub enum Route {
    Forward,
    /// Drops it, and tells the sender with `error` and this code.
    Drop(&'static str),
    /// Drops it and says nothing, as a target that stopped answering.
    Swallow,
    /// Forwards it twice.
    Twice,
    /// Keeps it, and forwards it after the next frame of the same sender.
    Hold,
    /// Forwards it as if another machine sent it.
    From(String),
}

pub type Hook = Box<dyn FnMut(&str, &mut Send) -> Route + std::marker::Send>;

#[derive(Clone)]
struct Machine {
    id: String,
    name: String,
    token: String,
    key: Option<String>,
}

#[derive(Default)]
struct State {
    user_id: String,
    remote_access: bool,
    machines: Vec<Machine>,
    sockets: HashMap<String, mpsc::UnboundedSender<Message>>,
    hook: Option<Hook>,
    mitm: Option<Mitm>,
    budgets: HashMap<String, RelayBudget>,
    refusals: u32,
    lease_requests: u32,
    /// Routes that answer `401` with this raw body instead: `me`, `key`,
    /// `lease` or `connect`.
    unauthorized: HashMap<&'static str, String>,
    held: HashMap<String, Value>,
    /// Every frame a daemon sent, with its sender.
    log: Vec<(String, Send)>,
    upgrades: Vec<(String, Instant)>,
    /// What `presence` claims for every machine instead, as a relay that
    /// alters it could.
    claimed_proto: Option<Value>,
    lease_status: u16,
    lease_ttl: u64,
    /// How long each lease takes to come.
    lease_delay: std::time::Duration,
    connections: u32,
}

#[derive(Clone)]
pub struct FakeCloud {
    pub base: String,
    state: Arc<Mutex<State>>,
}

impl FakeCloud {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            user_id: "u1".into(),
            remote_access: true,
            lease_status: 200,
            lease_ttl: 60,
            ..State::default()
        }));
        let cloud = Self { base, state };
        let serving = cloud.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let cloud = serving.clone();
                tokio::spawn(async move { cloud.serve(stream).await });
            }
        });
        cloud
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// A machine of the account; returns its token.
    pub fn add_machine(&self, id: &str, name: &str) -> String {
        let token = format!("lfy_{:064x}", self.state().machines.len() + 1);
        self.state().machines.push(Machine {
            id: id.into(),
            name: name.into(),
            token: token.clone(),
            key: None,
        });
        token
    }

    /// Makes every `presence` claim `{min, max}`, whatever the machines
    /// said when they connected.
    pub fn claim_proto(&self, min: u32, max: u32) {
        self.state().claimed_proto = Some(json!({"min": min, "max": max}));
    }

    pub fn set_hook(&self, hook: Option<Hook>) {
        self.state().hook = hook;
    }

    pub fn set_lease(&self, status: u16, ttl: u64) {
        let mut state = self.state();
        state.lease_status = status;
        state.lease_ttl = ttl;
    }

    /// Each lease takes this long to come.
    pub fn set_lease_delay(&self, delay: std::time::Duration) {
        self.state().lease_delay = delay;
    }

    pub fn set_remote_access(&self, remote_access: bool) {
        self.state().remote_access = remote_access;
    }

    /// Replaces a machine's key in the roster, as a cloud that lies would.
    pub fn set_roster_key(&self, id: &str, key: &str) {
        if let Some(machine) = self.state().machines.iter_mut().find(|m| m.id == id) {
            machine.key = Some(key.into());
        }
    }

    pub fn frames(&self) -> Vec<(String, Send)> {
        self.state().log.clone()
    }

    pub fn upgrades(&self) -> Vec<(String, Instant)> {
        self.state().upgrades.clone()
    }

    /// Answers `route` (`me`, `key`, `lease`, `connect`) with a `401` and
    /// this raw body, as a proxy or a captive portal could; `None` stops.
    pub fn set_unauthorized(&self, route: &'static str, body: Option<&str>) {
        let mut state = self.state();
        match body {
            Some(body) => state.unauthorized.insert(route, body.to_string()),
            None => state.unauthorized.remove(route),
        };
    }

    /// Whether the machine holds a socket now.
    pub fn connected(&self, id: &str) -> bool {
        self.state().sockets.contains_key(id)
    }

    pub fn lease_requests(&self) -> u32 {
        self.state().lease_requests
    }

    /// Frames the relay's budgets refused.
    pub fn refusals(&self) -> u32 {
        self.state().refusals
    }

    /// Puts the relay in the middle of every session between `a` (the
    /// viewer) and `b`: it answers each side's handshake with its own static
    /// key, as a relay that swapped keys in the roster would.
    pub fn set_mitm(&self, a: &str, b: &str) {
        self.state().mitm = Some(Mitm {
            a: a.into(),
            b: b.into(),
            keys: StaticKeys::generate().unwrap(),
            sessions: HashMap::new(),
            completed_with_b: 0,
        });
    }

    /// How many handshakes the relay completed with `b` as `a`.
    pub fn mitm_completed(&self) -> u32 {
        self.state()
            .mitm
            .as_ref()
            .map_or(0, |mitm| mitm.completed_with_b)
    }

    pub fn connections(&self) -> u32 {
        self.state().connections
    }

    /// Closes every socket with `code`, as a deploy does with `1012`.
    pub fn close_all(&self, code: u16) {
        for socket in self.state().sockets.values() {
            let _ = socket.send(close(code, ""));
        }
    }

    pub fn close(&self, machine: &str, code: u16, reason: &str) {
        if let Some(socket) = self.state().sockets.get(machine) {
            let _ = socket.send(close(code, reason));
        }
    }

    /// Sends a frame to a machine as if the relay routed it.
    pub fn inject(&self, to: &str, frame: Value) {
        if let Some(socket) = self.state().sockets.get(to) {
            let _ = socket.send(Message::text(frame.to_string()));
        }
    }

    /// `DELETE /v1/machines/{id}`: signs it out and tells the others.
    pub fn revoke(&self, id: &str) {
        let mut state = self.state();
        state.machines.retain(|machine| machine.id != id);
        if let Some(socket) = state.sockets.remove(id) {
            let _ = socket.send(close(4401, ""));
        }
        let frame = json!({"t": "revoked", "machineId": id}).to_string();
        for socket in state.sockets.values() {
            let _ = socket.send(Message::text(frame.clone()));
        }
    }

    async fn serve(self, stream: TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut head = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            head.push(line.trim_end().to_string());
        }
        let request_line = head.first().cloned().unwrap_or_default();
        let header = |name: &str| {
            head.iter().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
        };
        let token = header("authorization")
            .and_then(|value| value.strip_prefix("Bearer ").map(str::to_string))
            .unwrap_or_default();
        let length: usize = header("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; length];
        if length > 0 && reader.read_exact(&mut body).await.is_err() {
            return;
        }
        let machine = self
            .state()
            .machines
            .iter()
            .find(|m| m.token == token)
            .cloned();
        let mut stream = reader.into_inner();
        let route = match request_line.split_whitespace().nth(1).unwrap_or_default() {
            "/v1/me" => "me",
            "/v1/machines/self/remote-key" => "key",
            "/v1/remote/lease" => "lease",
            "/v1/remote/connect" => "connect",
            _ => "",
        };
        let unauthorized = self.state().unauthorized.get(route).cloned();
        if let Some(body) = unauthorized {
            if route == "lease" {
                self.state().lease_requests += 1;
            }
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return;
        }
        if request_line.starts_with("GET /v1/remote/connect") {
            let proto = header("x-localify-proto").and_then(|value| {
                let (min, max) = value.split_once('-')?;
                let (min, max): (u32, u32) = (min.parse().ok()?, max.parse().ok()?);
                (1..=max)
                    .contains(&min)
                    .then(|| json!({"min": min, "max": max}))
            });
            let speaks = header("sec-websocket-protocol").as_deref() == Some("localify-remote.1")
                && proto.is_some()
                && header("x-localify-version").is_some();
            if !speaks {
                let _ = stream
                    .write_all(b"HTTP/1.1 426 Upgrade Required\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await;
                return;
            }
            let key = header("sec-websocket-key").unwrap_or_default();
            return self.upgrade(stream, machine, &key).await;
        }
        if route == "lease" {
            let delay = self.state().lease_delay;
            tokio::time::sleep(delay).await;
        }
        let (status, answer) = self.answer(&request_line, machine.as_ref(), &body);
        let text = answer.map(|value| value.to_string()).unwrap_or_default();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
            text.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    }

    fn answer(
        &self,
        request: &str,
        machine: Option<&Machine>,
        body: &[u8],
    ) -> (&'static str, Option<Value>) {
        let Some(machine) = machine else {
            return (
                "401 Unauthorized",
                Some(json!({"error": "unauthorized", "message": "No."})),
            );
        };
        let mut state = self.state();
        if request.starts_with("GET /v1/me ") {
            let me = json!({
                "user": {"id": state.user_id, "login": "teo"},
                "machine": {"id": machine.id, "name": machine.name},
                "plan": {"status": "active", "entitled": true,
                         "limits": {"servingMachines": 1, "publicUrls": 3, "remoteAccess": state.remote_access},
                         "usage": {"servingMachines": 0, "publicUrls": 0}},
            });
            return ("200 OK", Some(me));
        }
        if request.starts_with("GET /v1/machines ") {
            let machines: Vec<Value> = state
                .machines
                .iter()
                .map(|m| {
                    json!({"id": m.id, "name": m.name, "createdAt": "2026-11-01T00:00:00Z",
                                "remoteKey": m.key, "self": m.id == machine.id})
                })
                .collect();
            return ("200 OK", Some(json!({ "machines": machines })));
        }
        if request.starts_with("PUT /v1/machines/self/remote-key ") {
            let key = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| v["publicKey"].as_str().map(str::to_string));
            let entry = state.machines.iter_mut().find(|m| m.id == machine.id);
            return match (entry, key) {
                (Some(entry), Some(key)) if entry.key.as_deref().is_none_or(|k| k == key) => {
                    entry.key = Some(key);
                    ("204 No Content", None)
                }
                _ => (
                    "409 Conflict",
                    Some(json!({"error": "remote_key_set", "message": "Set."})),
                ),
            };
        }
        if request.starts_with("GET /v1/remote/lease ") {
            state.lease_requests += 1;
            let refused = |code: &str| Some(json!({"error": code, "message": "Refused."}));
            match state.lease_status {
                200 => {}
                401 => return ("401 Unauthorized", refused("unauthorized")),
                402 => return ("402 Payment Required", refused("subscription_required")),
                403 => return ("403 Forbidden", refused("banned")),
                503 => return ("503 Service Unavailable", refused("unavailable")),
                _ => return ("500 Internal Server Error", refused("internal")),
            }
            let machines: Vec<&str> = state.machines.iter().map(|m| m.id.as_str()).collect();
            return (
                "200 OK",
                Some(json!({"ttl": state.lease_ttl, "machines": machines})),
            );
        }
        (
            "404 Not Found",
            Some(json!({"error": "not_found", "message": "No route."})),
        )
    }

    async fn upgrade(self, mut stream: TcpStream, machine: Option<Machine>, key: &str) {
        let Some(machine) = machine else {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
            return;
        };
        self.state()
            .upgrades
            .push((machine.id.clone(), Instant::now()));
        if !self.state().remote_access {
            let body = r#"{"error":"subscription_required","message":"Pro."}"#;
            let response = format!(
                "HTTP/1.1 402 Payment Required\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            return;
        }
        let accept = derive_accept_key(key.as_bytes());
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: localify-remote.1\r\n\r\n"
        );
        if stream.write_all(response.as_bytes()).await.is_err() {
            return;
        }
        let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        self.relay(socket, machine.id).await;
    }

    async fn relay(self, socket: WebSocketStream<TcpStream>, id: String) {
        let (mut sink, mut stream) = socket.split();
        let (sender, mut outgoing) = mpsc::unbounded_channel::<Message>();
        tokio::spawn(async move {
            while let Some(message) = outgoing.recv().await {
                let closing = matches!(message, Message::Close(_));
                if sink.send(message).await.is_err() || closing {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let peers = {
            let mut state = self.state();
            state.connections += 1;
            if let Some(old) = state.sockets.insert(id.clone(), sender.clone()) {
                let _ = old.send(close(4001, ""));
            }
            let claimed = state.claimed_proto.clone();
            let presence = presence_value(&id, "online", claimed.as_ref()).to_string();
            let others: Vec<Value> = state
                .sockets
                .iter()
                .filter(|(machine, _)| **machine != id)
                .map(|(machine, socket)| {
                    let _ = socket.send(Message::text(presence.clone()));
                    presence_value(machine, "online", claimed.as_ref())
                })
                .collect();
            others
        };
        let welcome =
            json!({"t": "welcome", "v": 1, "connId": "c", "machineId": id, "peers": peers});
        let _ = sender.send(Message::text(welcome.to_string()));
        while let Some(Ok(message)) = stream.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            if text.as_str() == "ping" {
                let _ = sender.send(Message::text("pong"));
                continue;
            }
            let Ok(frame) = serde_json::from_str::<Send>(text.as_str()) else {
                let _ = sender.send(close(4000, ""));
                break;
            };
            self.route(&id, &sender, frame);
        }
        let mut state = self.state();
        if state
            .sockets
            .get(&id)
            .is_some_and(|socket| socket.same_channel(&sender))
        {
            state.sockets.remove(&id);
            let gone = presence_value(&id, "offline", state.claimed_proto.as_ref()).to_string();
            for socket in state.sockets.values() {
                let _ = socket.send(Message::text(gone.clone()));
            }
        }
    }

    fn route(&self, from: &str, sender: &mpsc::UnboundedSender<Message>, mut frame: Send) {
        let mut state = self.state();
        state.log.push((from.to_string(), frame.clone()));
        // relay.ts's budgets, checked as a frame arrives. This relay handles
        // a frame at once, so none waits behind another.
        let size = serde_json::to_string(&frame).map_or(0, |text| text.len());
        let budget = state
            .budgets
            .entry(from.to_string())
            .or_insert_with(|| RelayBudget::new(Instant::now()));
        match budget.admit(size, 0, 0, Instant::now()) {
            Admit::Forward => {}
            Admit::Refuse => {
                state.refusals += 1;
                let error = json!({"t": "error", "code": "rate_limited", "sid": frame.sid, "message": "Slow down.", "retryAfter": 1});
                let _ = sender.send(Message::text(error.to_string()));
                return;
            }
            Admit::Close => {
                state.refusals += 1;
                let _ = sender.send(close(4429, r#"{"retryAfter":10}"#));
                return;
            }
        }
        if let Some(handled) = self.in_the_middle(&mut state, from, &frame) {
            if let Some((to, value)) = handled
                && let Some(socket) = state.sockets.get(&to)
            {
                let _ = socket.send(Message::text(value.to_string()));
            }
            return;
        }
        let route = match state.hook.as_mut() {
            Some(hook) => hook(from, &mut frame),
            None => Route::Forward,
        };
        let Some(target) = state.sockets.get(&frame.to).cloned() else {
            let error = json!({"t": "error", "code": "peer_offline", "sid": frame.sid, "message": "Offline."});
            let _ = sender.send(Message::text(error.to_string()));
            return;
        };
        let recv = |from: &str, frame: &Send| {
            let mut value =
                json!({"t": "recv", "from": from, "sid": frame.sid, "kind": frame.kind});
            if let Some(purpose) = frame.purpose {
                value["purpose"] = json!(purpose);
            }
            if let Some(data) = &frame.data {
                value["data"] = json!(data);
            }
            value
        };
        let deliver = |value: &Value| {
            let _ = target.send(Message::text(value.to_string()));
        };
        match route {
            Route::Forward => {
                deliver(&recv(from, &frame));
                if let Some(held) = state.held.remove(from) {
                    deliver(&held);
                }
            }
            Route::Twice => {
                let value = recv(from, &frame);
                deliver(&value);
                deliver(&value);
            }
            Route::Hold => {
                state.held.insert(from.to_string(), recv(from, &frame));
            }
            Route::From(other) => deliver(&recv(&other, &frame)),
            Route::Swallow => {}
            Route::Drop(code) => {
                let error = json!({"t": "error", "code": code, "sid": frame.sid, "message": "Dropped.", "retryAfter": 0.3});
                let _ = sender.send(Message::text(error.to_string()));
            }
        }
    }
}

struct Mitm {
    a: String,
    b: String,
    keys: StaticKeys,
    /// By session id: the relay as B towards A, and as A towards B.
    sessions: HashMap<String, (Handshake, Handshake)>,
    completed_with_b: u32,
}

type Delivery = Option<(String, Value)>;

impl FakeCloud {
    /// Runs the relay's two handshakes for a session between the pair it
    /// sits between; `None` for any other frame.
    fn in_the_middle(&self, state: &mut State, from: &str, frame: &Send) -> Option<Delivery> {
        let user = state.user_id.clone();
        let mitm = state.mitm.as_mut()?;
        let (a, b) = (mitm.a.clone(), mitm.b.clone());
        if !((from == a && frame.to == b) || (from == b && frame.to == a)) {
            return None;
        }
        let data = frame
            .data
            .as_deref()
            .and_then(|data| URL_SAFE_NO_PAD.decode(data).ok());
        let recv = |from: &str, purpose: Option<Purpose>, data: &[u8]| {
            let mut value = json!({"t": "recv", "from": from, "sid": frame.sid, "kind": "hs",
                                   "data": URL_SAFE_NO_PAD.encode(data)});
            if let Some(purpose) = purpose {
                value["purpose"] = json!(purpose);
            }
            value
        };
        match (from == a, frame.kind, frame.purpose, data) {
            // A's first message: answer it as B, and open towards B as A.
            (true, FrameKind::Hs, Some(purpose), Some(first)) => {
                let context = SessionContext {
                    purpose,
                    account: user,
                    initiator: a.clone(),
                    responder: b.clone(),
                    sid: SessionId::parse(&frame.sid)?,
                };
                let mut as_b = Handshake::responder(&mitm.keys, &context).ok()?;
                as_b.read(&first).ok()?;
                let second = as_b.write().ok()?;
                let mut as_a = Handshake::initiator(&mitm.keys, &context).ok()?;
                let towards_b = as_a.write().ok()?;
                mitm.sessions.insert(frame.sid.clone(), (as_b, as_a));
                if let Some(socket) = state.sockets.get(&b) {
                    let _ = socket.send(Message::text(
                        recv(&a, Some(purpose), &towards_b).to_string(),
                    ));
                }
                Some(Some((a.clone(), recv(&b, None, &second))))
            }
            // B's second message: finish the handshake with B, as A.
            (false, FrameKind::Hs, None, Some(second)) => {
                let (_, as_a) = mitm.sessions.get_mut(&frame.sid)?;
                as_a.read(&second).ok()?;
                let third = as_a.write().ok()?;
                if as_a.is_finished() {
                    mitm.completed_with_b += 1;
                }
                Some(Some((b.clone(), recv(&a, None, &third))))
            }
            // Anything else between them goes nowhere.
            _ => Some(None),
        }
    }
}

/// relay.ts's two budgets: 200 frames refilled at 50 a second, 5 MiB
/// refilled at 512 KiB a second, and at most 32 frames or 512 KiB waiting
/// behind the frame being handled. A refused frame takes nothing; the 50th
/// refusal in a row closes the socket with `4429`.
pub struct RelayBudget {
    frames: f64,
    bytes: f64,
    updated: Instant,
    refused_in_a_row: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Forward,
    Refuse,
    Close,
}

impl RelayBudget {
    const FRAMES: f64 = 200.0;
    const FRAME_RATE: f64 = 50.0;
    const BYTES: f64 = 5.0 * 1024.0 * 1024.0;
    const BYTE_RATE: f64 = 512.0 * 1024.0;
    pub const MAX_PENDING_FRAMES: usize = 32;
    pub const MAX_PENDING_BYTES: usize = 512 * 1024;

    pub fn new(now: Instant) -> Self {
        Self {
            frames: Self::FRAMES,
            bytes: Self::BYTES,
            updated: now,
            refused_in_a_row: 0,
        }
    }

    pub fn admit(
        &mut self,
        size: usize,
        pending_frames: usize,
        pending_bytes: usize,
        now: Instant,
    ) -> Admit {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.updated = now;
        self.frames = (self.frames + elapsed * Self::FRAME_RATE).min(Self::FRAMES);
        self.bytes = (self.bytes + elapsed * Self::BYTE_RATE).min(Self::BYTES);
        let full = pending_frames >= Self::MAX_PENDING_FRAMES
            || pending_bytes + size > Self::MAX_PENDING_BYTES;
        if full || self.frames < 1.0 || self.bytes < size as f64 {
            self.refused_in_a_row += 1;
            return if self.refused_in_a_row >= 50 {
                Admit::Close
            } else {
                Admit::Refuse
            };
        }
        self.refused_in_a_row = 0;
        self.frames -= 1.0;
        self.bytes -= size as f64;
        Admit::Forward
    }
}

/// `presence` for `id`: the versions this build speaks, unless the relay
/// claims others.
fn presence_value(id: &str, state: &str, claimed: Option<&Value>) -> Value {
    let proto = claimed
        .cloned()
        .unwrap_or_else(|| json!({"min": 1, "max": 2}));
    json!({"t": "presence", "machineId": id, "state": state, "version": "test",
           "proto": proto, "since": "2026-11-08T10:00:00Z"})
}

fn close(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.to_string().into(),
    }))
}

/// Whether a frame is a session message, not a handshake or a close.
pub fn is_message(frame: &Send) -> bool {
    frame.kind == FrameKind::Msg
}

/// Whether a frame opens a session.
pub fn opens(frame: &Send, purpose: Purpose) -> bool {
    frame.kind == FrameKind::Hs && frame.purpose == Some(purpose)
}

/// What a target shares in tests: the items its snapshots hold, numbered as
/// the daemon numbers them, or the services of its [`FakeHost`] projected
/// with the switches; the output folders of its runs; and its controls, run
/// by the real executor over the fake host, with request entries on disk.
#[derive(Default)]
pub struct FakeSource {
    items: Mutex<Option<Vec<InventoryItem>>>,
    revisions: Mutex<Option<Revisions>>,
    runs: Mutex<HashMap<String, Result<PathBuf, RunLookup>>>,
    builds: AtomicUsize,
    /// How long each build takes, in milliseconds.
    build_ms: AtomicUsize,
    /// Whether the desktop app runs.
    app_running: AtomicBool,
    host: OnceLock<Arc<FakeHost>>,
    runner: OnceLock<ControlRunner>,
    /// Where request entries are kept: the daemon's `services/requests`.
    requests: Mutex<Option<PathBuf>>,
    scratch: OnceLock<tempfile::TempDir>,
    /// The shell environment background starts would get, once a test
    /// gives one.
    shell: OnceLock<Arc<ShellEnvironment>>,
}

/// The executor's delays, shortened.
pub fn fast_controls() -> ControlTimings {
    ControlTimings {
        dispatch_budget: std::time::Duration::from_millis(1500),
        settle: std::time::Duration::from_millis(1000),
        poll: std::time::Duration::from_millis(20),
        ahead: std::time::Duration::from_secs(60),
    }
}

impl FakeSource {
    /// Keeps request entries in `dir`, as a daemon keeps them in its state
    /// folder: a daemon started again on it finds them.
    pub fn keep_requests_in(&self, dir: PathBuf) {
        *self.requests.lock().unwrap() = Some(dir);
    }

    /// The shell environment background starts read, as the daemon's.
    pub fn set_shell(&self, shell: ShellEnvironment) -> Arc<ShellEnvironment> {
        Arc::clone(self.shell.get_or_init(|| Arc::new(shell)))
    }

    /// Each build takes this long, so requests pile up behind it.
    pub fn set_build_time(&self, ms: usize) {
        self.build_ms.store(ms, Ordering::SeqCst);
    }

    pub fn set_items(&self, items: Vec<InventoryItem>) {
        *self.items.lock().unwrap() = Some(items);
    }

    /// `run_id` is its service's last run, with its output in `dir`.
    pub fn set_run(&self, run_id: &str, found: Result<PathBuf, RunLookup>) {
        self.runs.lock().unwrap().insert(run_id.into(), found);
    }

    /// The desktop app opens or quits.
    pub fn set_app(&self, app: AppPresence) {
        self.app_running
            .store(app == AppPresence::Running, Ordering::SeqCst);
    }

    /// How many snapshots were built.
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }

    /// The services the controls act on, and the snapshots show.
    pub fn host(&self) -> Arc<FakeHost> {
        Arc::clone(self.host.get_or_init(|| {
            Arc::new(FakeHost {
                state: Mutex::new(HostState::default()),
            })
        }))
    }

    fn runner(&self) -> &ControlRunner {
        self.runner.get_or_init(|| {
            let dir = self.requests.lock().unwrap().clone().unwrap_or_else(|| {
                self.scratch
                    .get_or_init(|| tempfile::tempdir().unwrap())
                    .path()
                    .to_path_buf()
            });
            let host: Arc<dyn Host> = self.host();
            ControlRunner::new(host, RequestEntries::new(dir), fast_controls())
        })
    }
}

impl LocalSource for FakeSource {
    fn snapshot(&self, switches: Switches) -> SnapshotFuture {
        self.builds.fetch_add(1, Ordering::SeqCst);
        let items = self.items.lock().unwrap().clone().unwrap_or_else(|| {
            let context = Context {
                settings: switches.settings,
                app: switches.app,
                tunnel_connected: false,
            };
            self.host()
                .facts()
                .into_iter()
                .map(|facts| inventory_item(facts, &context))
                .collect()
        });
        let snapshot = self
            .revisions
            .lock()
            .unwrap()
            .get_or_insert_with(|| Revisions::new("epoch-1".into()))
            .snapshot(items);
        let delay = std::time::Duration::from_millis(self.build_ms.load(Ordering::SeqCst) as u64);
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            snapshot
        })
    }

    fn run_output(&self, run_id: &RunId) -> anyhow::Result<Result<PathBuf, RunLookup>> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .get(run_id.as_str())
            .cloned()
            .unwrap_or(Err(RunLookup::Gone)))
    }

    fn app(&self) -> AppPresence {
        if self.app_running.load(Ordering::SeqCst) {
            AppPresence::Running
        } else {
            AppPresence::Absent
        }
    }

    fn control(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        self.runner().run(order, live, progress)
    }

    fn control_status(&self, peer: &str, request_id: &str) -> Report {
        let host = self.host();
        let held = {
            let mut state = host.state();
            state.statuses += 1;
            state.status_hold.take()
        };
        if let Some(held) = held {
            let _ = held.recv();
        }
        self.runner().status(peer, request_id)
    }

    fn control_admits(&self, peer: &str, request_id: &str) -> Result<(), Report> {
        self.runner().admits(peer, request_id)
    }

    fn shell_environment_failure(&self) -> Option<String> {
        self.shell.get().and_then(|shell| shell_failure(shell))
    }

    fn reload_shell_environment(&self) -> Option<String> {
        let shell = self.shell.get()?;
        shell.reload();
        shell_failure(shell)
    }
}

/// What the app bridge does with a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bridge {
    #[default]
    Accept,
    /// Spawns the run, and the answer is lost.
    Unanswered,
    Absent,
    Refuse(&'static str),
}

/// One service on the fake target.
#[derive(Debug, Clone)]
pub struct FakeService {
    pub facts: ServiceFacts,
    /// The legacy run the service's record still names, proven or not.
    pub record_names: Option<String>,
}

#[derive(Default)]
pub struct HostState {
    pub services: Vec<FakeService>,
    pub bridge: Bridge,
    /// A started run exits at once with this code instead of listening.
    pub exit_code: Option<i32>,
    pub launches: HashMap<String, LaunchReport>,
    /// What reached the app, and what was spawned detached.
    pub handed: Vec<LaunchRequest>,
    pub detached: Vec<DetachedLaunch>,
    pub stops: Vec<Expected>,
    /// Holds the next launch once it reached the host, until the test
    /// sends or drops the other end.
    pub hold: Option<std::sync::mpsc::Receiver<()>>,
    /// Holds the next stop the same way, before it ends anything.
    pub stop_hold: Option<std::sync::mpsc::Receiver<()>>,
    /// Holds the next `control.status` the same way, before it reads.
    pub status_hold: Option<std::sync::mpsc::Receiver<()>>,
    /// How many `control.status` queries reached the executor.
    pub statuses: usize,
}

/// A target's services in memory: stops end their run, launches run
/// `localify dev`'s own checks under the lock (a run still live, or a
/// record that no longer names the legacy run relaunched, refuses), then
/// start a run that listens.
pub struct FakeHost {
    state: Mutex<HostState>,
}

impl FakeHost {
    pub fn state(&self) -> MutexGuard<'_, HostState> {
        self.state.lock().unwrap()
    }

    pub fn facts(&self) -> Vec<ServiceFacts> {
        self.state()
            .services
            .iter()
            .map(|service| service.facts.clone())
            .collect()
    }

    /// The facts of the service named `id`.
    pub fn service(&self, id: &str) -> ServiceFacts {
        self.state()
            .services
            .iter()
            .find(|service| service.facts.service.id == id)
            .map(|service| service.facts.clone())
            .unwrap()
    }

    pub fn change(&self, id: &str, change: impl FnOnce(&mut FakeService)) {
        let mut state = self.state();
        let service = state
            .services
            .iter_mut()
            .find(|service| service.facts.service.id == id)
            .unwrap();
        change(service);
    }

    /// What `localify dev` does with a launch, under the service's lock.
    fn start(
        &self,
        id: &RunId,
        key: &ServiceKey,
        origin: RunOrigin,
        replaces_legacy: Option<&str>,
        profile: Option<String>,
    ) {
        let mut state = self.state();
        let exit_code = state.exit_code;
        let Some(service) = state
            .services
            .iter_mut()
            .find(|service| matches(service, key))
        else {
            return;
        };
        let live = service
            .facts
            .run
            .as_ref()
            .is_some_and(|run| run.process != RunProcess::Stopped);
        let renamed =
            replaces_legacy.is_some_and(|legacy| service.record_names.as_deref() != Some(legacy));
        let report = if live || renamed {
            LaunchReport::Refused {
                code: RefusalCode::Conflict,
                message: "Someone else started it meanwhile.".into(),
            }
        } else {
            service.record_names = None;
            service.facts.run = Some(RunFacts {
                run_id: Some(id.to_string()),
                origin,
                legacy: false,
                has_stream: true,
                process: if exit_code.is_some() {
                    RunProcess::Stopped
                } else {
                    RunProcess::Running
                },
                started_at: "2026-11-09T10:00:00Z".into(),
                profile,
                profile_danger: false,
                exit: None,
                legacy_id: None,
            });
            service.facts.reachable = Some(exit_code.is_none());
            match exit_code {
                Some(code) => LaunchReport::Exited {
                    code: Some(code),
                    signal: None,
                    reason: None,
                },
                None => LaunchReport::Running,
            }
        };
        state.launches.insert(id.to_string(), report);
    }
}

fn matches(service: &FakeService, key: &ServiceKey) -> bool {
    service.facts.project.id == key.project_id
        && service.facts.service.id == key.service_id
        && service.facts.environment.slug == key.environment
}

/// Nothing to kill or reap.
struct NoProcess;

impl Spawned for NoProcess {
    fn settle(self: Box<Self>, _: bool) {}
}

impl Host for FakeHost {
    fn look(
        &self,
        wanted: &ServiceKey,
        profile: Option<&str>,
        legacy_id: Option<&str>,
    ) -> anyhow::Result<Option<Look>> {
        let state = self.state();
        let Some(service) = state
            .services
            .iter()
            .find(|service| matches(service, wanted))
        else {
            return Ok(None);
        };
        Ok(Some(Look {
            facts: service.facts.clone(),
            key: wanted.clone(),
            profile_allowed: !matches!(profile, Some("prod" | "removed")),
            names_legacy: legacy_id.is_some() && service.record_names.as_deref() == legacy_id,
        }))
    }

    fn stop(&self, key: &ServiceKey, expected: &Expected) -> Result<(), Report> {
        let hold = self.state().stop_hold.take();
        if let Some(hold) = hold {
            let _ = hold.recv();
        }
        let mut state = self.state();
        state.stops.push(expected.clone());
        let Some(service) = state
            .services
            .iter_mut()
            .find(|service| matches(service, key))
        else {
            return Err(Report::refused("conflict", "Gone."));
        };
        let run = service
            .facts
            .run
            .as_mut()
            .filter(|run| run.process != RunProcess::Stopped);
        let Some(run) = run else {
            return Err(Report::refused("conflict", "Nothing runs."));
        };
        match expected {
            Expected::Run(run_id) if run.run_id.as_deref() != Some(run_id.as_str()) => {
                return Err(Report::refused("conflict", "Another run."));
            }
            Expected::Legacy(legacy_id) if service.record_names.as_deref() != Some(legacy_id) => {
                return Err(Report::refused("conflict", "Another run."));
            }
            Expected::Legacy(legacy_id) if run.legacy_id.as_deref() != Some(legacy_id) => {
                return Err(Report::refused("legacy_run", "Not proven."));
            }
            _ => {}
        }
        run.process = RunProcess::Stopped;
        service.facts.reachable = Some(false);
        Ok(())
    }

    fn launch_in_app(&self, request: &LaunchRequest) -> Handoff {
        let (bridge, hold) = {
            let mut state = self.state();
            state.handed.push(request.clone());
            (state.bridge, state.hold.take())
        };
        if let Some(hold) = hold {
            let _ = hold.recv();
        }
        let key = ServiceKey {
            project_id: request.service.project_id.clone(),
            environment: request.service.environment.clone(),
            service_id: request.service.service_id.clone(),
        };
        match bridge {
            Bridge::Absent => return Handoff::Absent,
            Bridge::Refuse(code) => {
                return Handoff::Refused(BridgeError {
                    code: code.into(),
                    message: "Refused.".into(),
                });
            }
            Bridge::Accept | Bridge::Unanswered => self.start(
                &request.launch_id,
                &key,
                RunOrigin::App,
                request.replaces_legacy.as_deref(),
                request.profile.clone(),
            ),
        }
        match bridge {
            Bridge::Unanswered => Handoff::Unanswered("The answer was lost.".into()),
            _ => Handoff::Accepted,
        }
    }

    fn launch_detached(&self, launch: &DetachedLaunch) -> anyhow::Result<Box<dyn Spawned>> {
        let hold = {
            let mut state = self.state();
            state.detached.push(launch.clone());
            state.hold.take()
        };
        if let Some(hold) = hold {
            let _ = hold.recv();
        }
        self.start(
            &launch.launch_id,
            &launch.key,
            RunOrigin::Background,
            launch.replaces_legacy.as_deref(),
            launch.profile.clone(),
        );
        Ok(Box::new(NoProcess))
    }

    fn report(&self, launch_id: &RunId, expires_at: Option<u64>) -> anyhow::Result<LaunchReport> {
        let mut state = self.state();
        if let Some(report) = state.launches.get(launch_id.as_str()) {
            return Ok(report.clone());
        }
        if expires_at.is_some_and(|expires| unix_ms_now() >= expires) {
            state
                .launches
                .insert(launch_id.to_string(), LaunchReport::Expired);
            return Ok(LaunchReport::Expired);
        }
        Ok(LaunchReport::Unknown)
    }

    fn listens(&self, port: u16) -> bool {
        self.state()
            .services
            .iter()
            .any(|service| service.facts.port == port && service.facts.reachable == Some(true))
    }

    fn last_lines(&self, _: &RunId, _: usize) -> String {
        "the last line".into()
    }
}
