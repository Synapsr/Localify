use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, warn};

use crate::protocol::{
    self, Frame, RESPONSE_END, RESPONSE_HEAD, RequestHead, ResponseHead, WS_BINARY, WS_CLOSE,
    WS_TEXT, WsClose,
};
use crate::session::{Outbound, send};
use crate::upstream::{Upstream, forwarded_headers};

type UpstreamSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens the upstream WebSocket for one public WebSocket (for example Vite
/// HMR) and relays messages both ways until either side closes.
pub async fn proxy(
    stream_id: u32,
    head: RequestHead,
    from_edge: mpsc::UnboundedReceiver<Message>,
    upstream: Upstream,
    out: Outbound,
) {
    let local_host = upstream.local_host(head.label.as_deref());
    let url = format!("ws://{}{}", upstream.address(), head.path);
    let mut request = match url.into_client_request() {
        Ok(request) => request,
        Err(error) => {
            let _ = respond(&out, stream_id, 400, Vec::new()).await;
            warn!(stream_id, %error, "invalid WebSocket request");
            return;
        }
    };
    for (name, value) in forwarded_headers(&head, &local_host) {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            request.headers_mut().insert(name, value);
        }
    }

    let socket = match connect_async(request).await {
        Ok((socket, response)) => {
            let headers = response
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|value| value.to_str().ok())
                .map(|protocol| vec![("sec-websocket-protocol".to_string(), protocol.to_string())])
                .unwrap_or_default();
            if respond(&out, stream_id, 101, headers).await.is_err() {
                return;
            }
            socket
        }
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status().as_u16();
            warn!(stream_id, status, host = %local_host, "upstream refused the WebSocket");
            let _ = respond(&out, stream_id, status, Vec::new()).await;
            let _ = send(&out, protocol::encode(RESPONSE_END, stream_id, &[])).await;
            return;
        }
        Err(error) => {
            warn!(stream_id, %error, host = %local_host, "upstream WebSocket failed");
            let _ = respond(&out, stream_id, 502, Vec::new()).await;
            let _ = send(&out, protocol::encode(RESPONSE_END, stream_id, &[])).await;
            return;
        }
    };

    debug!(stream_id, path = %head.path, "WebSocket open");
    relay(stream_id, socket, from_edge, &out).await;
    debug!(stream_id, "WebSocket closed");
}

async fn relay(
    stream_id: u32,
    socket: UpstreamSocket,
    mut from_edge: mpsc::UnboundedReceiver<Message>,
    out: &Outbound,
) {
    let (mut sink, mut incoming) = socket.split();
    loop {
        tokio::select! {
            message = incoming.next() => {
                let frame = match message {
                    Some(Ok(Message::Text(text))) => {
                        protocol::encode(WS_TEXT, stream_id, text.as_bytes())
                    }
                    Some(Ok(Message::Binary(data))) => protocol::encode(WS_BINARY, stream_id, &data),
                    Some(Ok(Message::Close(frame))) => {
                        let _ = send(out, close_frame(stream_id, frame)).await;
                        return;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) | None => {
                        let _ = send(out, close_frame(stream_id, None)).await;
                        return;
                    }
                };
                if send(out, frame).await.is_err() {
                    return;
                }
            }
            message = from_edge.recv() => match message {
                Some(message) => {
                    let closing = matches!(message, Message::Close(_));
                    if sink.send(message).await.is_err() || closing {
                        return;
                    }
                }
                None => return,
            },
        }
    }
}

/// Converts an edge frame into a message for the upstream socket.
pub fn message_from_frame(frame: &Frame) -> Option<Message> {
    match frame.kind {
        WS_TEXT => String::from_utf8(frame.payload.to_vec())
            .ok()
            .map(|text| Message::Text(text.into())),
        WS_BINARY => Some(Message::Binary(frame.payload.clone())),
        WS_CLOSE => {
            let close: WsClose = serde_json::from_slice(&frame.payload).ok()?;
            Some(Message::Close(Some(CloseFrame {
                code: CloseCode::from(close.code),
                reason: close.reason.into(),
            })))
        }
        _ => None,
    }
}

fn close_frame(stream_id: u32, frame: Option<CloseFrame>) -> bytes::Bytes {
    let close = match frame {
        Some(frame) => WsClose {
            code: u16::from(frame.code),
            reason: frame.reason.to_string(),
        },
        None => WsClose {
            code: 1011,
            reason: "Local app closed the WebSocket".into(),
        },
    };
    protocol::encode_json(WS_CLOSE, stream_id, &close).unwrap_or_else(|_| {
        protocol::encode(WS_CLOSE, stream_id, b"{\"code\":1011,\"reason\":\"\"}")
    })
}

async fn respond(
    out: &Outbound,
    stream_id: u32,
    status: u16,
    headers: Vec<(String, String)>,
) -> anyhow::Result<()> {
    let head = ResponseHead { status, headers };
    send(out, protocol::encode_json(RESPONSE_HEAD, stream_id, &head)?).await
}
