use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tracing::{info, warn};

use crate::Config;
use crate::protocol::{self, Frame, RequestHead};
use crate::upstream::{self, Upstream};
use crate::websocket;

const CONNECT_PATH: &str = "/_localify/connect";
/// Bounded so a slow edge socket pushes back on upstream response reads.
const OUTBOUND_CAPACITY: usize = 256;
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);

/// Frames queued for the edge socket.
pub type Outbound = mpsc::Sender<Message>;

pub async fn send(out: &Outbound, frame: Bytes) -> Result<()> {
    out.send(Message::Binary(frame))
        .await
        .context("connector session closed")
}

enum StreamHandle {
    Http {
        /// `None` once the edge sent the end of the request body.
        body: Option<mpsc::UnboundedSender<Bytes>>,
        task: AbortHandle,
    },
    WebSocket {
        to_upstream: mpsc::UnboundedSender<Message>,
        task: AbortHandle,
    },
}

impl StreamHandle {
    fn abort(&self) {
        match self {
            Self::Http { task, .. } | Self::WebSocket { task, .. } => task.abort(),
        }
    }
}

/// Runs one connector session until the edge socket closes.
pub async fn run(config: &Config, upstream: &Upstream) -> Result<()> {
    let url = format!("{}{CONNECT_PATH}?slug={}", config.edge_url, config.slug);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.token))?,
    );
    // Nagle would hold back the small frames that follow a response head.
    let (socket, _) = connect_async_with_config(request, None, true)
        .await
        .context("could not connect to the edge")?;
    info!(slug = %config.slug, upstream = %config.upstream, "connector online");

    let (mut sink, mut incoming) = socket.split();
    let (out, mut out_rx) = mpsc::channel::<Message>(OUTBOUND_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<u32>();
    let mut streams = HashMap::new();
    let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);

    let result = loop {
        tokio::select! {
            message = incoming.next() => match message {
                Some(Ok(Message::Binary(data))) => match Frame::decode(data) {
                    Ok(frame) => dispatch(frame, &mut streams, upstream, &out, &done_tx),
                    Err(error) => warn!(%error, "dropping malformed frame"),
                },
                Some(Ok(Message::Close(_))) | None => break Ok(()),
                Some(Ok(_)) => {}
                Some(Err(error)) => break Err(error.into()),
            },
            Some(stream_id) = done_rx.recv() => {
                streams.remove(&stream_id);
            }
            _ = keepalive.tick() => {
                if out.send(Message::Ping(Bytes::new())).await.is_err() {
                    break Ok(());
                }
            }
        }
    };

    for handle in streams.values() {
        handle.abort();
    }
    writer.abort();
    result
}

fn dispatch(
    frame: Frame,
    streams: &mut HashMap<u32, StreamHandle>,
    upstream: &Upstream,
    out: &Outbound,
    done: &mpsc::UnboundedSender<u32>,
) {
    let stream_id = frame.stream_id;
    match frame.kind {
        protocol::REQUEST_HEAD => {
            let head: RequestHead = match serde_json::from_slice(&frame.payload) {
                Ok(head) => head,
                Err(error) => {
                    warn!(stream_id, %error, "invalid request head");
                    return;
                }
            };
            let handle = if head.websocket {
                let (to_upstream, from_edge) = mpsc::unbounded_channel();
                let work =
                    websocket::proxy(stream_id, head, from_edge, upstream.clone(), out.clone());
                let task = spawn_stream(stream_id, done.clone(), work);
                StreamHandle::WebSocket { to_upstream, task }
            } else {
                let (body_tx, body_rx) = mpsc::unbounded_channel();
                let body = head.has_body.then_some(body_tx);
                let work = upstream::proxy(stream_id, head, body_rx, upstream.clone(), out.clone());
                let task = spawn_stream(stream_id, done.clone(), work);
                StreamHandle::Http { body, task }
            };
            streams.insert(stream_id, handle);
        }
        protocol::REQUEST_BODY => {
            if let Some(StreamHandle::Http {
                body: Some(body), ..
            }) = streams.get(&stream_id)
            {
                let _ = body.send(frame.payload);
            }
        }
        protocol::REQUEST_END => {
            if let Some(StreamHandle::Http { body, .. }) = streams.get_mut(&stream_id) {
                *body = None;
            }
        }
        protocol::RESET => {
            if let Some(handle) = streams.remove(&stream_id) {
                handle.abort();
            }
        }
        protocol::WS_TEXT | protocol::WS_BINARY | protocol::WS_CLOSE => {
            if let Some(StreamHandle::WebSocket { to_upstream, .. }) = streams.get(&stream_id)
                && let Some(message) = websocket::message_from_frame(&frame)
            {
                let _ = to_upstream.send(message);
            }
        }
        other => warn!(kind = other, stream_id, "unknown frame type"),
    }
}

fn spawn_stream(
    stream_id: u32,
    done: mpsc::UnboundedSender<u32>,
    work: impl Future<Output = ()> + Send + 'static,
) -> AbortHandle {
    tokio::spawn(async move {
        work.await;
        let _ = done.send(stream_id);
    })
    .abort_handle()
}
