//! The one outbound WebSocket to the relay (`docs/remote-machines.md`,
//! "Connecting"). TLS goes through the same rustls setup as the HTTP client;
//! plain `ws://` only reaches a loopback `wrangler dev` or a test relay,
//! since the API base was checked when the machine signed in.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Uri};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Error as WsError};

use super::backoff::Ending;
use crate::infrastructure::remote::wire::{MAX_FRAME_BYTES, PROTO_MAX, PROTO_MIN};

pub const SUBPROTOCOL: &str = "localify-remote.1";
const CONNECT_PATH: &str = "/v1/remote/connect";
/// DNS, TCP, TLS and the upgrade together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub type RelaySocket = WebSocketStream<Box<dyn Io>>;

/// Connects to `<api base>/v1/remote/connect` with the machine's token.
pub async fn connect(api_base: &str, token: &str) -> Result<RelaySocket, Ending> {
    match tokio::time::timeout(CONNECT_TIMEOUT, connect_once(api_base, token)).await {
        Ok(result) => result,
        Err(_) => Err(Ending::Unreachable),
    }
}

async fn connect_once(api_base: &str, token: &str) -> Result<RelaySocket, Ending> {
    let uri: Uri = api_base.parse().map_err(|_| Ending::Unreachable)?;
    let secure = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(Ending::Unreachable),
    };
    let host = uri.host().ok_or(Ending::Unreachable)?.to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let authority = uri
        .authority()
        .map(|authority| authority.as_str().to_string())
        .ok_or(Ending::Unreachable)?;
    let scheme = if secure { "wss" } else { "ws" };
    let request = request(&format!("{scheme}://{authority}{CONNECT_PATH}"), token)?;

    let tcp = TcpStream::connect((host.trim_matches(['[', ']']), port))
        .await
        .map_err(|_| Ending::Unreachable)?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = if secure {
        Box::new(tls(tcp, &host).await?)
    } else {
        Box::new(tcp)
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    match tokio_tungstenite::client_async_with_config(request, stream, Some(config)).await {
        Ok((socket, _)) => Ok(socket),
        Err(WsError::Http(response)) => Err(refusal(&response)),
        Err(_) => Err(Ending::Unreachable),
    }
}

fn request(url: &str, token: &str) -> Result<tungstenite::handshake::client::Request, Ending> {
    let mut request = url.into_client_request().map_err(|_| Ending::Unreachable)?;
    let headers = request.headers_mut();
    let header = |value: &str| HeaderValue::from_str(value).map_err(|_| Ending::Unreachable);
    headers.insert("Authorization", header(&format!("Bearer {token}"))?);
    headers.insert("Sec-WebSocket-Protocol", header(SUBPROTOCOL)?);
    headers.insert("X-Localify-Version", header(env!("CARGO_PKG_VERSION"))?);
    headers.insert(
        "X-Localify-Proto",
        header(&format!("{PROTO_MIN}-{PROTO_MAX}"))?,
    );
    headers.insert(
        "User-Agent",
        header(concat!("localify/", env!("CARGO_PKG_VERSION")))?,
    );
    Ok(request)
}

async fn tls(
    tcp: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Ending> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // An explicit provider, as the HTTP client uses: the desktop app never
    // installs a process-wide default one.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| Ending::Unreachable)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| Ending::Unreachable)?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|_| Ending::Unreachable)
}

/// The status of a refused upgrade, its `Retry-After`, and the API's error
/// code from the body.
fn refusal(response: &tungstenite::http::Response<Option<Vec<u8>>>) -> Ending {
    let retry_after = response
        .headers()
        .get("Retry-After")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(3600)));
    let code = response
        .body()
        .as_deref()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
        .and_then(|body| body.get("error")?.as_str().map(str::to_string));
    Ending::Refused {
        status: response.status().as_u16(),
        code,
        retry_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upgrade_carries_the_token_subprotocol_and_version() {
        let request = request("wss://api.localify.dev/v1/remote/connect", "lfy_t").unwrap();
        let headers = request.headers();
        assert_eq!(headers["Authorization"], "Bearer lfy_t");
        assert_eq!(headers["Sec-WebSocket-Protocol"], "localify-remote.1");
        assert_eq!(headers["X-Localify-Version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(headers["X-Localify-Proto"], "1-2");
    }

    #[test]
    fn a_refused_upgrade_keeps_its_status_code_and_hint() {
        let response = tungstenite::http::Response::builder()
            .status(429)
            .header("Retry-After", "12")
            .body(Some(
                br#"{"error":"rate_limited","message":"Slow down."}"#.to_vec(),
            ))
            .unwrap();
        assert_eq!(
            refusal(&response),
            Ending::Refused {
                status: 429,
                code: Some("rate_limited".into()),
                retry_after: Some(Duration::from_secs(12)),
            }
        );
    }
}
