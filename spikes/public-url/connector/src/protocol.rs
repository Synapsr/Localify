//! Wire protocol between the connector and the `PublicEndpoint` Durable Object.
//! Must stay in sync with `edge/src/protocol.ts`.
//!
//! Every WebSocket message is one binary frame: a type byte, a big-endian
//! `u32` stream id, then the payload.

use anyhow::{Result, bail};
use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

pub const REQUEST_HEAD: u8 = 1;
pub const REQUEST_BODY: u8 = 2;
pub const REQUEST_END: u8 = 3;
pub const RESPONSE_HEAD: u8 = 4;
pub const RESPONSE_BODY: u8 = 5;
pub const RESPONSE_END: u8 = 6;
pub const RESET: u8 = 7;
pub const WS_TEXT: u8 = 8;
pub const WS_BINARY: u8 = 9;
pub const WS_CLOSE: u8 = 10;

/// Body chunks are split so one large stream cannot monopolize the socket.
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;

const HEADER_BYTES: usize = 5;

#[derive(Debug)]
pub struct Frame {
    pub kind: u8,
    pub stream_id: u32,
    pub payload: Bytes,
}

impl Frame {
    pub fn decode(data: Bytes) -> Result<Self> {
        if data.len() < HEADER_BYTES {
            bail!("frame too short: {} bytes", data.len());
        }
        let stream_id = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        Ok(Self {
            kind: data[0],
            stream_id,
            payload: data.slice(HEADER_BYTES..),
        })
    }
}

pub fn encode(kind: u8, stream_id: u32, payload: &[u8]) -> Bytes {
    let mut buf = BytesMut::with_capacity(HEADER_BYTES + payload.len());
    buf.put_u8(kind);
    buf.put_u32(stream_id);
    buf.put_slice(payload);
    buf.freeze()
}

pub fn encode_json(kind: u8, stream_id: u32, value: &impl Serialize) -> Result<Bytes> {
    Ok(encode(kind, stream_id, &serde_json::to_vec(value)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub public_host: String,
    pub label: Option<String>,
    pub has_body: bool,
    pub websocket: bool,
}

#[derive(Debug, Serialize)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsClose {
    pub code: u16,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let encoded = encode(RESPONSE_BODY, 0x0102_0304, b"hello");
        let frame = Frame::decode(encoded).unwrap();
        assert_eq!(frame.kind, RESPONSE_BODY);
        assert_eq!(frame.stream_id, 0x0102_0304);
        assert_eq!(&frame.payload[..], b"hello");
    }

    #[test]
    fn short_frame_is_rejected() {
        assert!(Frame::decode(Bytes::from_static(&[1, 0, 0])).is_err());
    }

    #[test]
    fn request_head_matches_edge_json() {
        let json = r#"{"method":"GET","path":"/a?b=1","headers":[["accept","*/*"]],"publicHost":"x--y.localhost","label":"x","hasBody":false,"websocket":false}"#;
        let head: RequestHead = serde_json::from_str(json).unwrap();
        assert_eq!(
            head.headers,
            vec![("accept".to_string(), "*/*".to_string())]
        );
        assert_eq!(head.label.as_deref(), Some("x"));
    }
}
