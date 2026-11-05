use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, StreamBody};
use hyper::body::Frame as BodyFrame;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::sync::{Semaphore, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, warn};

use crate::protocol::{
    self, MAX_CHUNK_BYTES, RESPONSE_BODY, RESPONSE_END, RESPONSE_HEAD, RequestHead, ResponseHead,
};
use crate::session::{Outbound, send};

type UpstreamBody = BoxBody<Bytes, Infallible>;

/// Upstream requests in flight at once. Over HTTP/2 a browser sends a whole
/// module graph at once, and one new connection per request overflows the
/// dev server's accept backlog (128 on macOS): the extra connections fail or
/// stall. Browsers cap HTTP/1.1 at 6 connections per host.
const MAX_IN_FLIGHT: usize = 32;

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The local app every public request is forwarded to.
#[derive(Clone)]
pub struct Upstream {
    inner: Arc<Inner>,
}

struct Inner {
    address: String,
    local_domain: String,
    client: Client<HttpConnector, UpstreamBody>,
    in_flight: Arc<Semaphore>,
}

impl Upstream {
    pub fn new(address: String, local_domain: String) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self {
            inner: Arc::new(Inner {
                address,
                local_domain,
                client,
                in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            }),
        }
    }

    pub fn address(&self) -> &str {
        &self.inner.address
    }

    /// Maps a wildcard label back to its local domain (ADR 0015).
    pub fn local_host(&self, label: Option<&str>) -> String {
        match label {
            Some(label) => format!("{label}.{}", self.inner.local_domain),
            None => self.inner.local_domain.clone(),
        }
    }
}

/// Request headers for the upstream app: the local domain in `Host` and the
/// public host in `X-Forwarded-Host` (ADR 0017).
pub fn forwarded_headers(head: &RequestHead, local_host: &str) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = head
        .headers
        .iter()
        .filter(|(name, _)| should_forward(name))
        .cloned()
        .collect();
    let client_ip = head
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("cf-connecting-ip"))
        .map(|(_, value)| value.clone());

    headers.push(("host".into(), local_host.into()));
    headers.push(("x-forwarded-host".into(), head.public_host.clone()));
    headers.push(("x-forwarded-proto".into(), "https".into()));
    headers.push(("x-localify-local-domain".into(), local_host.into()));
    if let Some(client_ip) = client_ip {
        headers.push(("x-forwarded-for".into(), client_ip));
    }
    headers
}

fn should_forward(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !(HOP_BY_HOP.contains(&name.as_str())
        || name == "host"
        || name == "x-real-ip"
        || name.starts_with("x-forwarded-")
        || name.starts_with("x-localify-")
        || name.starts_with("cf-"))
}

pub fn response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| !HOP_BY_HOP.contains(&name.as_str()))
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect()
}

/// Serves one public HTTP request against the upstream and streams the
/// response back to the edge.
pub async fn proxy(
    stream_id: u32,
    head: RequestHead,
    body: mpsc::UnboundedReceiver<Bytes>,
    upstream: Upstream,
    out: Outbound,
) {
    let started = Instant::now();
    let local_host = upstream.local_host(head.label.as_deref());
    let request = match build_request(&head, &local_host, upstream.address(), body) {
        Ok(request) => request,
        Err(error) => {
            let _ = send_error(&out, stream_id, 400, &format!("Invalid request: {error}")).await;
            return;
        }
    };

    // Held until the body ends, so it bounds open upstream connections. A
    // long-lived stream such as SSE keeps its permit for its whole life.
    let Ok(_permit) = upstream.inner.in_flight.clone().acquire_owned().await else {
        return;
    };
    let response = match upstream.inner.client.request(request).await {
        Ok(response) => response,
        Err(error) => {
            warn!(stream_id, %error, host = %local_host, "upstream request failed");
            let message = format!("Localify could not reach {local_host}: {error}");
            let _ = send_error(&out, stream_id, 502, &message).await;
            return;
        }
    };

    let (parts, mut body) = response.into_parts();
    let response_head = ResponseHead {
        status: parts.status.as_u16(),
        headers: response_headers(&parts.headers),
    };
    let Ok(frame) = protocol::encode_json(RESPONSE_HEAD, stream_id, &response_head) else {
        return;
    };
    if send(&out, frame).await.is_err() {
        return;
    }

    let mut bytes = 0usize;
    while let Some(next) = body.frame().await {
        match next {
            Ok(frame) => {
                let Ok(data) = frame.into_data() else {
                    continue; // Trailers are not forwarded in the spike.
                };
                bytes += data.len();
                for chunk in data.chunks(MAX_CHUNK_BYTES) {
                    if send(&out, protocol::encode(RESPONSE_BODY, stream_id, chunk))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(error) => {
                let reason = format!("Upstream body failed: {error}");
                let _ = send(
                    &out,
                    protocol::encode(protocol::RESET, stream_id, reason.as_bytes()),
                )
                .await;
                return;
            }
        }
    }
    let _ = send(&out, protocol::encode(RESPONSE_END, stream_id, &[])).await;
    debug!(
        stream_id,
        method = %head.method,
        path = %head.path,
        status = response_head.status,
        bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "proxied"
    );
}

fn build_request(
    head: &RequestHead,
    local_host: &str,
    address: &str,
    body: mpsc::UnboundedReceiver<Bytes>,
) -> anyhow::Result<Request<UpstreamBody>> {
    let method = Method::from_bytes(head.method.as_bytes())?;
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{address}{}", head.path));
    for (name, value) in forwarded_headers(head, local_host) {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) else {
            continue;
        };
        request = request.header(name, value);
    }

    let body = if head.has_body {
        let chunks = UnboundedReceiverStream::new(body).map(|chunk| Ok(BodyFrame::data(chunk)));
        BodyExt::boxed(StreamBody::new(chunks))
    } else {
        Empty::new().boxed()
    };
    Ok(request.body(body)?)
}

async fn send_error(
    out: &Outbound,
    stream_id: u32,
    status: u16,
    message: &str,
) -> anyhow::Result<()> {
    let head = ResponseHead {
        status,
        headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())],
    };
    send(out, protocol::encode_json(RESPONSE_HEAD, stream_id, &head)?).await?;
    send(
        out,
        protocol::encode(RESPONSE_BODY, stream_id, message.as_bytes()),
    )
    .await?;
    send(out, protocol::encode(RESPONSE_END, stream_id, &[])).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(headers: &[(&str, &str)]) -> RequestHead {
        RequestHead {
            method: "GET".into(),
            path: "/".into(),
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            public_host: "feature--app-a8f3.localify.fyi".into(),
            label: Some("feature".into()),
            has_body: false,
            websocket: false,
        }
    }

    fn value<'a>(headers: &'a [(String, String)], name: &str) -> Vec<&'a str> {
        headers
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    #[test]
    fn local_host_maps_wildcard_label() {
        let upstream = Upstream::new("127.0.0.1:1".into(), "app.localify".into());
        assert_eq!(upstream.local_host(Some("feature")), "feature.app.localify");
        assert_eq!(upstream.local_host(None), "app.localify");
    }

    #[test]
    fn forwarded_headers_use_local_host_and_drop_spoofed_forwarding() {
        let headers = forwarded_headers(
            &head(&[
                ("accept", "text/html"),
                ("x-forwarded-host", "evil.example"),
                ("x-localify-local-domain", "evil.localify"),
                ("cf-connecting-ip", "203.0.113.7"),
                ("connection", "keep-alive"),
            ]),
            "feature.app.localify",
        );
        assert_eq!(value(&headers, "host"), ["feature.app.localify"]);
        assert_eq!(
            value(&headers, "x-forwarded-host"),
            ["feature--app-a8f3.localify.fyi"]
        );
        assert_eq!(
            value(&headers, "x-localify-local-domain"),
            ["feature.app.localify"]
        );
        assert_eq!(value(&headers, "x-forwarded-for"), ["203.0.113.7"]);
        assert_eq!(value(&headers, "accept"), ["text/html"]);
        assert!(value(&headers, "connection").is_empty());
        assert!(value(&headers, "cf-connecting-ip").is_empty());
    }
}
