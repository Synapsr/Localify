//! A Localify Cloud in the test process: the routes the link calls, and a
//! relay that routes frames as `docs/remote-machines.md`, "Relay", says
//! (`from` set from the socket, `peer_offline` for a machine with no
//! socket, `welcome` and `presence`), with hooks that drop, repeat, hold or
//! rewrite frames the way a misbehaving relay could.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

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

use crate::infrastructure::remote::wire::{FrameKind, Purpose, Send};

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
    held: HashMap<String, Value>,
    /// Every frame a daemon sent, with its sender.
    log: Vec<(String, Send)>,
    upgrades: Vec<(String, Instant)>,
    lease_status: u16,
    lease_ttl: u64,
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

    pub fn set_hook(&self, hook: Option<Hook>) {
        self.state().hook = hook;
    }

    pub fn set_lease(&self, status: u16, ttl: u64) {
        let mut state = self.state();
        state.lease_status = status;
        state.lease_ttl = ttl;
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
        if request_line.starts_with("GET /v1/remote/connect") {
            let speaks = header("sec-websocket-protocol").as_deref() == Some("localify-remote.1")
                && header("x-localify-proto").as_deref() == Some("1-1")
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
            if state.lease_status != 200 {
                return (
                    "402 Payment Required",
                    Some(json!({"error": "subscription_required", "message": "Pro."})),
                );
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
            let presence = presence_value(&id, "online").to_string();
            let others: Vec<Value> = state
                .sockets
                .iter()
                .filter(|(machine, _)| **machine != id)
                .map(|(machine, socket)| {
                    let _ = socket.send(Message::text(presence.clone()));
                    presence_value(machine, "online")
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
            let gone = presence_value(&id, "offline").to_string();
            for socket in state.sockets.values() {
                let _ = socket.send(Message::text(gone.clone()));
            }
        }
    }

    fn route(&self, from: &str, sender: &mpsc::UnboundedSender<Message>, mut frame: Send) {
        let mut state = self.state();
        state.log.push((from.to_string(), frame.clone()));
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

fn presence_value(id: &str, state: &str) -> Value {
    json!({"t": "presence", "machineId": id, "state": state, "version": "test",
           "proto": {"min": 1, "max": 1}, "since": "2026-11-08T10:00:00Z"})
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
