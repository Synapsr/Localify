use std::net::IpAddr;
use std::time::Instant;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode, Uri, header, header::HeaderName, header::HeaderValue},
    response::{IntoResponse, Response},
};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use crate::branding::BRANDING;
use crate::domain::ProxyTarget;

use super::theme;

/// Non-standard (but de facto standard) forwarding header names.
/// The `http` crate only provides constants for IANA-registered headers,
/// so we define these ourselves to avoid scattered string literals.
const X_FORWARDED_FOR: &str = "x-forwarded-for";
const X_FORWARDED_HOST: &str = "x-forwarded-host";
const X_FORWARDED_PROTO: &str = "x-forwarded-proto";
const KEEP_ALIVE: &str = "keep-alive";

#[derive(Clone, Copy)]
enum ProxyErrorKind {
    Connect,
    InvalidTargetUri,
    UpstreamIo,
    WebSocketUpgrade,
}

/// Scheme of the original client request (injected by server layers).
#[derive(Clone, Copy)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// Client IP address (injected by server layers).
#[derive(Clone, Copy)]
pub struct ClientAddr(pub IpAddr);

/// Build the `X-Forwarded-For` value by appending the client IP to any existing chain.
fn build_xff_value(existing: Option<&str>, client_ip: IpAddr) -> String {
    match existing {
        Some(chain) => format!("{}, {}", chain, client_ip),
        None => client_ip.to_string(),
    }
}

/// Check if request is a WebSocket upgrade
fn is_websocket_upgrade(request: &Request) -> bool {
    request
        .headers()
        .get(header::UPGRADE)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

/// Build HTTP upgrade request string to send to backend
fn build_upgrade_request(
    request: &Request,
    target: &ProxyTarget,
    host: &str,
    scheme: &str,
    client_ip: Option<IpAddr>,
) -> String {
    let path = request.uri().path();
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    let mut req = format!(
        "GET {}{} HTTP/1.1\r\nHost: {}:{}\r\n",
        path,
        query,
        target.host(),
        target.port()
    );

    // Forwarding headers
    req.push_str(&format!("X-Forwarded-Host: {}\r\n", host));
    req.push_str(&format!("X-Forwarded-Proto: {}\r\n", scheme));
    if let Some(ip) = client_ip {
        let existing = request
            .headers()
            .get(X_FORWARDED_FOR)
            .and_then(|v| v.to_str().ok());
        let xff = build_xff_value(existing, ip);
        req.push_str(&format!("X-Forwarded-For: {}\r\n", xff));
    }

    // Collect any extra hop-by-hop header names declared in the Connection header
    // (e.g. "Connection: X-Secret, keep-alive") so we can skip them below.
    // "upgrade" is excluded because the backend needs it for the WebSocket handshake.
    let dynamic_hop_by_hop: Vec<String> = request
        .headers()
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty() && t != "upgrade")
        .collect();

    // Copy remaining headers, skipping Host, forwarding headers, and hop-by-hop
    // headers (RFC 7230 §6.1). Connection and Upgrade are kept because the
    // backend needs them for the WebSocket handshake.
    for (name, value) in request.headers() {
        if name == header::HOST
            || name.as_str() == X_FORWARDED_HOST
            || name.as_str() == X_FORWARDED_PROTO
            || name.as_str() == X_FORWARDED_FOR
            || name == header::PROXY_AUTHENTICATE
            || name == header::PROXY_AUTHORIZATION
            || name == header::TE
            || name == header::TRAILER
            || name == header::TRANSFER_ENCODING
            || name.as_str() == KEEP_ALIVE
            || dynamic_hop_by_hop.contains(&name.as_str().to_ascii_lowercase())
        {
            continue;
        }
        if let Ok(v) = value.to_str() {
            req.push_str(&format!("{}: {}\r\n", name, v));
        }
    }
    req.push_str("\r\n");
    req
}

/// Proxy a WebSocket connection
async fn proxy_websocket(
    target: &ProxyTarget,
    request: Request,
    host: &str,
    scheme: &str,
    client_ip: Option<IpAddr>,
    project_root_path: Option<&str>,
) -> Response {
    // Connect to backend
    let backend_addr = format!("{}:{}", target.host(), target.port());
    debug!(target = %target, "Connecting to backend for WebSocket");
    let mut backend = match TcpStream::connect(&backend_addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!(target = %target, error = %e, "WebSocket backend connection failed");
            return build_proxy_error_response(
                StatusCode::BAD_GATEWAY,
                host,
                target,
                ProxyErrorKind::Connect,
                &e.to_string(),
                project_root_path,
            );
        }
    };
    let start_time = Instant::now();

    // Build and send the upgrade request to backend
    let upgrade_request = build_upgrade_request(&request, target, host, scheme, client_ip);

    if let Err(e) = backend.write_all(upgrade_request.as_bytes()).await {
        return build_proxy_error_response(
            StatusCode::BAD_GATEWAY,
            host,
            target,
            ProxyErrorKind::UpstreamIo,
            &format!("Backend write error: {e}"),
            project_root_path,
        );
    }
    debug!(target = %target, "WebSocket upgrade request sent to backend");

    // Read backend response (101 Switching Protocols expected)
    let mut response_buf = vec![0u8; 4096];
    let n = match backend.read(&mut response_buf).await {
        Ok(n) => n,
        Err(e) => {
            return build_proxy_error_response(
                StatusCode::BAD_GATEWAY,
                host,
                target,
                ProxyErrorKind::UpstreamIo,
                &format!("Backend read error: {e}"),
                project_root_path,
            );
        }
    };

    // Verify we got 101 Switching Protocols
    let response_str = String::from_utf8_lossy(&response_buf[..n]);
    let is_101 = response_str
        .lines()
        .next()
        .is_some_and(|line| line.contains("101"));
    if !is_101 {
        warn!(target = %target, "Backend rejected WebSocket upgrade");
        return build_proxy_error_response(
            StatusCode::BAD_GATEWAY,
            host,
            target,
            ProxyErrorKind::WebSocketUpgrade,
            "Backend rejected WebSocket upgrade",
            project_root_path,
        );
    }

    // Extract Sec-WebSocket-Accept from backend response
    let accept_key = response_str
        .lines()
        .find(|line| line.to_lowercase().starts_with("sec-websocket-accept:"))
        .and_then(|line| line.split(':').nth(1))
        .map(|s| s.trim().to_string());

    // Get the OnUpgrade handle from the request
    let on_upgrade = hyper::upgrade::on(request);

    info!(target = %target, "WebSocket connection established");

    // Clone target for the spawned task
    let target_str = target.to_string();

    // Spawn task to handle the bidirectional copy after upgrade
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let client = hyper_util::rt::TokioIo::new(upgraded);
                let (mut client_read, mut client_write) = tokio::io::split(client);
                let (mut backend_read, mut backend_write) = backend.into_split();

                let client_to_backend = async {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match client_read.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        if backend_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                };

                let backend_to_client = async {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match backend_read.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        if client_write.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                };

                let closed_by = tokio::select! {
                    _ = client_to_backend => "client",
                    _ = backend_to_client => "backend",
                };

                let duration = start_time.elapsed();
                info!(
                    target = %target_str,
                    duration_ms = duration.as_millis() as u64,
                    "WebSocket connection closed"
                );
                debug!(
                    target = %target_str,
                    closed_by = closed_by,
                    "WebSocket close details"
                );
            }
            Err(e) => {
                warn!(target = %target_str, error = %e, "WebSocket upgrade failed");
            }
        }
    });

    // Build 101 Switching Protocols response for client
    let mut builder = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::UPGRADE, "websocket")
        .header(header::CONNECTION, "Upgrade");

    if let Some(key) = accept_key {
        builder = builder.header("Sec-WebSocket-Accept", key);
    }

    builder.body(Body::empty()).unwrap_or_else(|_| {
        (
            StatusCode::BAD_GATEWAY,
            "Failed to build WebSocket upgrade response",
        )
            .into_response()
    })
}

/// Set `X-Forwarded-For`, `X-Forwarded-Proto`, and `X-Forwarded-Host` headers.
fn set_forwarding_headers(
    headers: &mut HeaderMap,
    host: &str,
    scheme: &str,
    client_ip: Option<IpAddr>,
) {
    if let Ok(value) = HeaderValue::from_str(host) {
        headers.insert(X_FORWARDED_HOST, value);
    }

    if let Ok(value) = HeaderValue::from_str(scheme) {
        headers.insert(X_FORWARDED_PROTO, value);
    }

    if let Some(ip) = client_ip {
        let existing = headers.get(X_FORWARDED_FOR).and_then(|v| v.to_str().ok());
        let xff = build_xff_value(existing, ip);
        if let Ok(hv) = HeaderValue::from_str(&xff) {
            headers.insert(X_FORWARDED_FOR, hv);
        }
    }

    debug!(
        x_forwarded_host = %host,
        x_forwarded_proto = %scheme,
        x_forwarded_for = %client_ip.map(|ip| ip.to_string()).unwrap_or_default(),
        "Forwarding headers set"
    );
}

/// Remove hop-by-hop headers that must not be forwarded (RFC 7230 §6.1).
///
/// Also strips any extra headers listed in the `Connection` header value.
fn strip_hop_by_hop_headers(headers: &mut HeaderMap) {
    // Collect headers named in all Connection values (e.g. "Connection: X-Custom, keep-alive").
    // Connection can legally appear multiple times (RFC 7230 §6.1).
    let connection_headers: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();

    headers.remove(header::CONNECTION);
    headers.remove(KEEP_ALIVE);
    headers.remove(header::PROXY_AUTHENTICATE);
    headers.remove(header::PROXY_AUTHORIZATION);
    headers.remove(header::TE);
    headers.remove(header::TRAILER);
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::UPGRADE);

    for name in connection_headers {
        headers.remove(&name);
    }
}

/// Proxy a request to a backend (supports HTTP/1.1, HTTP/2, and WebSocket)
pub async fn proxy_request(
    target: &ProxyTarget,
    request: Request,
    host: &str,
    scheme: &str,
    client_ip: Option<IpAddr>,
    project_root_path: Option<&str>,
) -> Response {
    // Check for WebSocket upgrade
    if is_websocket_upgrade(&request) {
        debug!(target = %target, "Proxying WebSocket request");
        return proxy_websocket(target, request, host, scheme, client_ip, project_root_path).await;
    }

    debug!(target = %target, "Proxying HTTP request");

    // Regular HTTP proxy
    let mut connector = HttpConnector::new();
    connector.set_nodelay(true);

    let client = Client::builder(TokioExecutor::new()).build(connector);

    // Rewrite the URI to target the backend
    let path = request.uri().path();
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let uri_string = format!(
        "http://{}:{}{}{}",
        target.host(),
        target.port(),
        path,
        query
    );

    let uri: Uri = match uri_string.parse() {
        Ok(u) => u,
        Err(e) => {
            return build_proxy_error_response(
                StatusCode::BAD_REQUEST,
                host,
                target,
                ProxyErrorKind::InvalidTargetUri,
                &format!("Invalid URI: {e}"),
                project_root_path,
            );
        }
    };

    let mut request = request;
    *request.uri_mut() = uri;

    // Set forwarding headers before removing Host
    set_forwarding_headers(request.headers_mut(), host, scheme, client_ip);

    // Remove original Host header (hyper client sets it for the target)
    request.headers_mut().remove(header::HOST);

    // Strip hop-by-hop headers
    strip_hop_by_hop_headers(request.headers_mut());

    // Forward the request
    match client.request(request).await {
        Ok(response) => {
            debug!(target = %target, status = %response.status(), "Proxy response");
            let (mut parts, body) = response.into_parts();
            strip_hop_by_hop_headers(&mut parts.headers);
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => {
            if e.is_connect() {
                warn!(target = %target, error = %e, "Backend connection failed");
                build_proxy_error_response(
                    StatusCode::BAD_GATEWAY,
                    host,
                    target,
                    ProxyErrorKind::Connect,
                    &e.to_string(),
                    project_root_path,
                )
            } else {
                warn!(target = %target, error = %e, "Proxy failed");
                build_proxy_error_response(
                    StatusCode::BAD_GATEWAY,
                    host,
                    target,
                    ProxyErrorKind::UpstreamIo,
                    &format!("Proxy error: {e}"),
                    project_root_path,
                )
            }
        }
    }
}

fn build_proxy_error_response(
    status: StatusCode,
    host: &str,
    target: &ProxyTarget,
    kind: ProxyErrorKind,
    error: &str,
    project_root_path: Option<&str>,
) -> Response {
    let html = render_proxy_error_page(host, target, kind, error, project_root_path);

    Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap_or_else(|_| {
            (status, format!("Proxy error for {} at {}", host, target)).into_response()
        })
}

fn render_proxy_error_page(
    host: &str,
    target: &ProxyTarget,
    kind: ProxyErrorKind,
    error: &str,
    project_root_path: Option<&str>,
) -> String {
    let domain = host.split(':').next().unwrap_or(host).trim_end_matches('.');
    let domain = theme::html_escape(domain);
    let target_display = theme::html_escape(&target.to_string());
    let error = theme::html_escape(error);
    let port = target.port().value();

    let mut body = String::new();
    body.push_str("<section class=\"px-error\">\n");

    // Hero
    body.push_str("<header class=\"px-error__hero\">\n");
    body.push_str("<span class=\"px-badge px-badge--danger\">");
    body.push_str(PROXY_ERROR_ICON);
    body.push_str("<span>");
    body.push_str(proxy_error_eyebrow(kind));
    body.push_str("</span></span>\n");
    body.push_str("<h1 class=\"px-error__title\">");
    body.push_str(proxy_error_title(kind));
    body.push_str("</h1>\n");
    body.push_str("<p class=\"px-error__lede\">");
    body.push_str(&proxy_error_summary(kind, &domain, &target_display));
    body.push_str("</p>\n");

    body.push_str("<dl class=\"px-meta\">\n");
    body.push_str("<div class=\"px-meta__item\"><dt>Domain</dt><dd><code>");
    body.push_str(&domain);
    body.push_str("</code></dd></div>\n");
    body.push_str("<div class=\"px-meta__item\"><dt>Route target</dt><dd><code>");
    body.push_str(&target_display);
    body.push_str("</code></dd></div>\n");
    body.push_str("</dl>\n");
    body.push_str("</header>\n");

    // Possible fixes
    body.push_str("<section class=\"px-panel\">\n");
    body.push_str("<div class=\"px-panel__head\">\n");
    body.push_str("<h2 class=\"px-panel__title\">Possible fixes</h2>\n");
    body.push_str("<p class=\"px-panel__hint\">Work through these in order &mdash; the first one is usually it.</p>\n");
    body.push_str("</div>\n");
    body.push_str("<ol class=\"px-fixes\">");
    append_proxy_fix_items(&mut body, kind, port);
    body.push_str("</ol>\n");
    body.push_str("</section>\n");

    // Project folder action
    if let Some(project_root_path) = project_root_path {
        let project_root_path = theme::html_escape(project_root_path);
        body.push_str("<section class=\"px-panel px-panel--accent\">\n");
        body.push_str("<div class=\"px-panel__head\">\n");
        body.push_str("<h2 class=\"px-panel__title\">Open the project test</h2>\n");
        body.push_str(
            "<p class=\"px-panel__hint\">Launch the linked folder in your local editor.</p>\n",
        );
        body.push_str("</div>\n");
        body.push_str("<div class=\"px-action\">\n");
        body.push_str(
            "<form method=\"post\" action=\"/.localify/open-in-editor\" class=\"px-action__form\">",
        );
        body.push_str("<button type=\"submit\" class=\"px-button px-button--primary\">");
        body.push_str(EDITOR_ICON);
        body.push_str("<span>Open in editor</span>");
        body.push_str("</button>");
        body.push_str("</form>\n");
        body.push_str("<code class=\"px-path\">");
        body.push_str(&project_root_path);
        body.push_str("</code>\n");
        body.push_str("</div>\n");
        body.push_str(
            "<p class=\"px-action__footnote\">Opens in Cursor or VS Code on this machine.</p>\n",
        );
        body.push_str("</section>\n");
    }

    // Diagnostics
    body.push_str("<section class=\"px-panel\">\n");
    body.push_str("<div class=\"px-panel__head\">\n");
    body.push_str("<h2 class=\"px-panel__title\">Diagnostics</h2>\n");
    body.push_str(
        "<p class=\"px-panel__hint\">Run these to confirm what is listening on this port.</p>\n",
    );
    body.push_str("</div>\n");
    body.push_str("<pre class=\"px-terminal\"><code>");
    body.push_str("<span class=\"px-terminal__prompt\">$</span> lsof -nP -iTCP:");
    body.push_str(&port.to_string());
    body.push_str(" -sTCP:LISTEN\n");
    body.push_str("<span class=\"px-terminal__prompt\">$</span> ");
    body.push_str(BRANDING.cli_command());
    body.push_str(" list");
    if matches!(kind, ProxyErrorKind::InvalidTargetUri) {
        body.push_str("\n<span class=\"px-terminal__prompt\">$</span> ");
        body.push_str(BRANDING.cli_command());
        body.push_str(" route add ");
        body.push_str(&domain);
        body.push_str(" / ");
        body.push_str(&port.to_string());
    }
    body.push_str("</code></pre>\n");
    body.push_str("</section>\n");

    // Raw error
    body.push_str("<details class=\"px-details\">\n");
    body.push_str("<summary class=\"px-details__summary\">");
    body.push_str("<span>Show raw proxy error</span>");
    body.push_str("<svg class=\"px-details__chevron\" width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><polyline points=\"6 9 12 15 18 9\"/></svg>");
    body.push_str("</summary>\n");
    body.push_str("<pre class=\"px-raw\">");
    body.push_str(&error);
    body.push_str("</pre>\n");
    body.push_str("</details>\n");

    body.push_str("</section>\n");

    theme::render_page(proxy_error_title(kind), &body, PROXY_ERROR_CSS, "")
}

fn proxy_error_eyebrow(kind: ProxyErrorKind) -> &'static str {
    match kind {
        ProxyErrorKind::Connect => "Upstream offline",
        ProxyErrorKind::InvalidTargetUri => "Route misconfigured",
        ProxyErrorKind::UpstreamIo => "Upstream broke the connection",
        ProxyErrorKind::WebSocketUpgrade => "WebSocket upgrade failed",
    }
}

fn proxy_error_title(kind: ProxyErrorKind) -> &'static str {
    match kind {
        ProxyErrorKind::Connect => "Service Unreachable aze",
        ProxyErrorKind::InvalidTargetUri => "Invalid Route Target",
        ProxyErrorKind::UpstreamIo => "Proxy Failed",
        ProxyErrorKind::WebSocketUpgrade => "WebSocket Upgrade Failed",
    }
}

fn proxy_error_summary(kind: ProxyErrorKind, domain: &str, target: &str) -> String {
    match kind {
        ProxyErrorKind::Connect => format!(
            "{} reached <code>{}</code> but could not connect to backend <code>{}</code>.",
            BRANDING.app_name, domain, target
        ),
        ProxyErrorKind::InvalidTargetUri => format!(
            "{} could not build a valid backend URL for <code>{}</code> from route target <code>{}</code>.",
            BRANDING.app_name, domain, target
        ),
        ProxyErrorKind::UpstreamIo => format!(
            "{} reached route target <code>{}</code> for <code>{}</code>, but the backend did not return a usable response.",
            BRANDING.app_name, target, domain
        ),
        ProxyErrorKind::WebSocketUpgrade => format!(
            "{} connected to <code>{}</code> for <code>{}</code>, but the WebSocket upgrade did not complete.",
            BRANDING.app_name, target, domain
        ),
    }
}

fn append_proxy_fix_items(body: &mut String, kind: ProxyErrorKind, port: u16) {
    match kind {
        ProxyErrorKind::Connect => {
            body.push_str("<li>Start your app or API dev server if it is not running.</li>");
            body.push_str("<li>Make sure the app listens on <code>127.0.0.1</code> or the exact host configured for this route.</li>");
            body.push_str("<li>If the app uses another port, update the route or your <code>package.json</code> dev script to use <code>");
            body.push_str(&port.to_string());
            body.push_str("</code>.</li>");
        }
        ProxyErrorKind::InvalidTargetUri => {
            body.push_str("<li>Check the route target saved in Localify. A malformed host or port can make the backend URL invalid.</li>");
            body.push_str("<li>Re-register or update the route with a plain port like <code>");
            body.push_str(&port.to_string());
            body.push_str("</code> or a valid <code>host:port</code> target.</li>");
            body.push_str("<li>If this came from project bootstrap, rerun <code>localify init-project</code> and confirm the selected target.</li>");
        }
        ProxyErrorKind::UpstreamIo => {
            body.push_str("<li>Restart the target service and retry the request.</li>");
            body.push_str("<li>Inspect the target app logs. The backend may be crashing or closing the connection early.</li>");
            body.push_str("<li>If a proxy or framework dev server sits in front of your app, verify it still listens on <code>");
            body.push_str(&port.to_string());
            body.push_str("</code>.</li>");
        }
        ProxyErrorKind::WebSocketUpgrade => {
            body.push_str("<li>Make sure the backend endpoint actually supports WebSocket upgrades on this path.</li>");
            body.push_str("<li>Check that the target dev server preserves <code>Upgrade</code> and <code>Connection</code> headers.</li>");
            body.push_str("<li>If the WebSocket server moved to another port, update the route or dev script to use <code>");
            body.push_str(&port.to_string());
            body.push_str("</code>.</li>");
        }
    }
}

const PROXY_ERROR_ICON: &str = concat!(
    "<svg width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" ",
    "stroke=\"currentColor\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">",
    "<path d=\"M12 9v4\"/><path d=\"M12 17h.01\"/>",
    "<path d=\"M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0Z\"/>",
    "</svg>",
);

const EDITOR_ICON: &str = concat!(
    "<svg width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" ",
    "stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">",
    "<path d=\"M15 3h6v6\"/><path d=\"M10 14 21 3\"/>",
    "<path d=\"M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6\"/>",
    "</svg>",
);

const PROXY_ERROR_CSS: &str = "\
.px-error{\
    max-width:720px;margin:0 auto;\
    display:flex;flex-direction:column;gap:20px;\
    animation:px-fade .5s ease-out;\
}\
@keyframes px-fade{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:translateY(0)}}\
\
.px-error__hero{\
    padding:32px;\
    background:var(--surface);\
    border:1px solid var(--border);\
    border-radius:var(--radius-lg);\
    box-shadow:var(--shadow-sm);\
}\
.px-error__title{\
    font-size:clamp(1.6rem,2.4vw,2rem);\
    line-height:1.15;letter-spacing:-.02em;\
    color:var(--ink);margin:14px 0 10px;\
    font-weight:700;\
}\
.px-error__lede{\
    color:var(--ink-soft);font-size:1rem;line-height:1.65;\
    margin:0 0 18px;max-width:60ch;\
}\
.px-error__lede code{font-size:.9em}\
\
.px-badge{\
    display:inline-flex;align-items:center;gap:.45rem;\
    padding:.35rem .7rem;border-radius:999px;\
    font-size:.72rem;font-weight:600;\
    letter-spacing:.08em;text-transform:uppercase;\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
}\
.px-badge--danger{\
    color:var(--danger-strong);\
    background:var(--danger-soft);\
    border:1px solid rgba(177,74,42,.18);\
}\
\
.px-meta{\
    display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));\
    gap:10px;margin:0;\
    border-top:1px solid var(--border);padding-top:18px;\
}\
.px-meta__item{margin:0}\
.px-meta dt{\
    font-size:.7rem;font-weight:600;\
    letter-spacing:.12em;text-transform:uppercase;\
    color:var(--ink-muted);margin:0 0 6px;\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
}\
.px-meta dd{margin:0;font-size:.92rem;color:var(--ink)}\
.px-meta code{word-break:break-all}\
\
.px-panel{\
    padding:24px 28px;\
    background:var(--surface);\
    border:1px solid var(--border);\
    border-radius:var(--radius-lg);\
    box-shadow:var(--shadow-sm);\
}\
.px-panel--accent{\
    background:linear-gradient(180deg,rgba(37,111,90,.06),rgba(37,111,90,.02));\
    border-color:rgba(37,111,90,.18);\
}\
.px-panel__head{margin-bottom:16px}\
.px-panel__title{\
    font-size:1rem;font-weight:600;color:var(--ink);\
    letter-spacing:-.01em;margin:0 0 4px;\
}\
.px-panel__hint{\
    margin:0;font-size:.88rem;color:var(--ink-muted);line-height:1.5;\
}\
\
.px-fixes{\
    list-style:none;padding:0;margin:0;\
    display:flex;flex-direction:column;gap:10px;\
    counter-reset:px-fix;\
}\
.px-fixes li{\
    position:relative;\
    padding:12px 14px 12px 44px;\
    background:var(--surface-soft);\
    border:1px solid var(--border);\
    border-radius:var(--radius-md);\
    font-size:.94rem;color:var(--ink-soft);line-height:1.55;\
}\
.px-fixes li::before{\
    counter-increment:px-fix;content:counter(px-fix);\
    position:absolute;left:12px;top:50%;transform:translateY(-50%);\
    width:22px;height:22px;border-radius:50%;\
    background:var(--accent-soft);color:var(--accent-strong);\
    display:flex;align-items:center;justify-content:center;\
    font-size:.72rem;font-weight:700;\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
}\
.px-fixes li code{font-size:.88em}\
\
.px-action{\
    display:flex;flex-wrap:wrap;align-items:center;gap:12px;\
}\
.px-action__form{margin:0}\
.px-button{\
    appearance:none;cursor:pointer;\
    display:inline-flex;align-items:center;gap:.5rem;\
    padding:10px 16px;border-radius:999px;\
    font:600 .9rem/1 ui-sans-serif,-apple-system,BlinkMacSystemFont,sans-serif;\
    transition:transform .15s ease,box-shadow .15s ease,background .15s ease;\
}\
.px-button--primary{\
    background:linear-gradient(135deg,#17342D,#2A6A58);\
    color:#F9F6EF;border:1px solid rgba(0,0,0,.1);\
    box-shadow:0 8px 20px rgba(20,46,39,.18);\
}\
.px-button--primary:hover{transform:translateY(-1px);box-shadow:0 12px 26px rgba(20,46,39,.24)}\
.px-button--primary:active{transform:translateY(0)}\
.px-path{\
    font-size:.85rem;padding:.35rem .65rem;\
    word-break:break-all;\
}\
.px-action__footnote{\
    margin:12px 0 0;font-size:.82rem;color:var(--ink-muted);\
}\
\
.px-terminal{\
    background:#0F1917;color:#E8F0EC;\
    padding:16px 18px;\
    border-radius:var(--radius-md);\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
    font-size:.85rem;line-height:1.8;\
    border:1px solid rgba(255,255,255,.06);\
    overflow-x:auto;margin:0;\
}\
.px-terminal code{background:transparent;color:inherit;padding:0;border:0;font-size:inherit}\
.px-terminal__prompt{color:#6BAF97;margin-right:.5rem;user-select:none}\
\
.px-details{\
    background:var(--surface);\
    border:1px solid var(--border);\
    border-radius:var(--radius-lg);\
    box-shadow:var(--shadow-sm);\
    overflow:hidden;\
}\
.px-details__summary{\
    display:flex;align-items:center;justify-content:space-between;\
    padding:16px 24px;cursor:pointer;\
    font-size:.88rem;font-weight:600;color:var(--ink-soft);\
    list-style:none;\
}\
.px-details__summary::-webkit-details-marker{display:none}\
.px-details__chevron{transition:transform .2s ease;color:var(--ink-muted)}\
.px-details[open] .px-details__chevron{transform:rotate(180deg)}\
.px-raw{\
    margin:0;padding:16px 24px 20px;\
    background:var(--danger-soft);\
    border-top:1px solid rgba(177,74,42,.14);\
    color:var(--danger-strong);\
    font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
    font-size:.82rem;line-height:1.6;\
    white-space:pre-wrap;word-break:break-word;\
}\
\
@media(max-width:600px){\
    .px-error__hero{padding:22px}\
    .px-panel{padding:18px 20px}\
    .px-details__summary{padding:14px 18px}\
    .px-raw{padding:14px 18px 16px}\
}\
";

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use axum::http::{HeaderMap, HeaderValue, Request, header};

    use crate::domain::ProxyTarget;

    const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    // --- Scheme ---

    #[test]
    fn scheme_as_str_returns_http() {
        assert_eq!(Scheme::Http.as_str(), "http");
    }

    #[test]
    fn scheme_as_str_returns_https() {
        assert_eq!(Scheme::Https.as_str(), "https");
    }

    // --- build_xff_value ---

    #[test]
    fn xff_no_existing_chain() {
        assert_eq!(build_xff_value(None, LOCALHOST), "127.0.0.1");
    }

    #[test]
    fn xff_appends_to_existing_chain() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(
            build_xff_value(Some("192.168.1.1"), ip),
            "192.168.1.1, 10.0.0.1"
        );
    }

    #[test]
    fn xff_appends_to_multi_hop_chain() {
        assert_eq!(
            build_xff_value(Some("1.1.1.1, 2.2.2.2"), LOCALHOST),
            "1.1.1.1, 2.2.2.2, 127.0.0.1"
        );
    }

    #[test]
    fn xff_with_ipv6() {
        let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(build_xff_value(None, ip), "::1");
    }

    // --- is_websocket_upgrade ---

    #[test]
    fn detects_websocket_upgrade() {
        let req = Request::builder()
            .header(header::UPGRADE, "websocket")
            .body(Body::empty())
            .unwrap();
        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn detects_websocket_upgrade_case_insensitive() {
        let req = Request::builder()
            .header(header::UPGRADE, "WebSocket")
            .body(Body::empty())
            .unwrap();
        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn not_websocket_without_upgrade_header() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn not_websocket_with_different_upgrade() {
        let req = Request::builder()
            .header(header::UPGRADE, "h2c")
            .body(Body::empty())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    // --- set_forwarding_headers ---

    #[test]
    fn forwarding_headers_sets_all_three() {
        let mut headers = HeaderMap::new();
        set_forwarding_headers(&mut headers, "myapp.roxy", "https", Some(LOCALHOST));

        assert_eq!(headers.get(X_FORWARDED_HOST).unwrap(), "myapp.roxy");
        assert_eq!(headers.get(X_FORWARDED_PROTO).unwrap(), "https");
        assert_eq!(headers.get(X_FORWARDED_FOR).unwrap(), "127.0.0.1");
    }

    #[test]
    fn forwarding_headers_without_client_ip() {
        let mut headers = HeaderMap::new();
        set_forwarding_headers(&mut headers, "myapp.roxy", "http", None);

        assert_eq!(headers.get(X_FORWARDED_HOST).unwrap(), "myapp.roxy");
        assert_eq!(headers.get(X_FORWARDED_PROTO).unwrap(), "http");
        assert!(headers.get(X_FORWARDED_FOR).is_none());
    }

    #[test]
    fn forwarding_headers_appends_to_existing_xff() {
        let mut headers = HeaderMap::new();
        headers.insert(X_FORWARDED_FOR, HeaderValue::from_static("10.0.0.1"));

        set_forwarding_headers(&mut headers, "myapp.roxy", "https", Some(LOCALHOST));

        assert_eq!(headers.get(X_FORWARDED_FOR).unwrap(), "10.0.0.1, 127.0.0.1");
    }

    // --- strip_hop_by_hop_headers ---

    #[test]
    fn strips_all_standard_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(KEEP_ALIVE, HeaderValue::from_static("timeout=5"));
        headers.insert(
            header::PROXY_AUTHENTICATE,
            HeaderValue::from_static("Basic"),
        );
        headers.insert(
            header::PROXY_AUTHORIZATION,
            HeaderValue::from_static("Basic abc"),
        );
        headers.insert(header::TE, HeaderValue::from_static("trailers"));
        headers.insert(header::TRAILER, HeaderValue::from_static("Expires"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(header::UPGRADE, HeaderValue::from_static("h2c"));
        // This one should survive
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));

        strip_hop_by_hop_headers(&mut headers);

        assert!(headers.get(header::CONNECTION).is_none());
        assert!(headers.get(KEEP_ALIVE).is_none());
        assert!(headers.get(header::PROXY_AUTHENTICATE).is_none());
        assert!(headers.get(header::PROXY_AUTHORIZATION).is_none());
        assert!(headers.get(header::TE).is_none());
        assert!(headers.get(header::TRAILER).is_none());
        assert!(headers.get(header::TRANSFER_ENCODING).is_none());
        assert!(headers.get(header::UPGRADE).is_none());
        assert_eq!(headers.get(header::CONTENT_TYPE).unwrap(), "text/html");
    }

    #[test]
    fn strips_dynamic_headers_named_in_connection() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("X-Custom, X-Secret"),
        );
        headers.insert("x-custom", HeaderValue::from_static("value1"));
        headers.insert("x-secret", HeaderValue::from_static("value2"));
        headers.insert("x-keep", HeaderValue::from_static("value3"));

        strip_hop_by_hop_headers(&mut headers);

        assert!(headers.get("x-custom").is_none());
        assert!(headers.get("x-secret").is_none());
        assert_eq!(headers.get("x-keep").unwrap(), "value3");
    }

    #[test]
    fn strips_dynamic_headers_from_multiple_connection_values() {
        let mut headers = HeaderMap::new();
        headers.append(header::CONNECTION, HeaderValue::from_static("X-First"));
        headers.append(header::CONNECTION, HeaderValue::from_static("X-Second"));
        headers.insert("x-first", HeaderValue::from_static("one"));
        headers.insert("x-second", HeaderValue::from_static("two"));
        headers.insert("x-keep", HeaderValue::from_static("three"));

        strip_hop_by_hop_headers(&mut headers);

        assert!(headers.get("x-first").is_none());
        assert!(headers.get("x-second").is_none());
        assert_eq!(headers.get("x-keep").unwrap(), "three");
    }

    #[test]
    fn strip_on_empty_headers_is_noop() {
        let mut headers = HeaderMap::new();
        strip_hop_by_hop_headers(&mut headers);
        assert!(headers.is_empty());
    }

    // --- build_upgrade_request ---

    fn make_target() -> ProxyTarget {
        ProxyTarget::parse("3000").unwrap()
    }

    fn ws_request(path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .header(header::HOST, "myapp.roxy")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .unwrap()
    }

    #[test]
    fn upgrade_request_contains_forwarding_headers() {
        let req = ws_request("/ws");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", Some(LOCALHOST));

        assert!(raw.contains("X-Forwarded-Host: myapp.roxy\r\n"));
        assert!(raw.contains("X-Forwarded-Proto: https\r\n"));
        assert!(raw.contains("X-Forwarded-For: 127.0.0.1\r\n"));
    }

    #[test]
    fn upgrade_request_omits_xff_without_client_ip() {
        let req = ws_request("/ws");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(raw.contains("X-Forwarded-Host: myapp.roxy\r\n"));
        assert!(raw.contains("X-Forwarded-Proto: https\r\n"));
        assert!(!raw.contains("X-Forwarded-For"));
    }

    #[test]
    fn upgrade_request_appends_to_existing_xff() {
        let req = Request::builder()
            .uri("/ws")
            .header(header::HOST, "myapp.roxy")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(X_FORWARDED_FOR, "10.0.0.1")
            .body(Body::empty())
            .unwrap();
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", Some(LOCALHOST));

        assert!(raw.contains("X-Forwarded-For: 10.0.0.1, 127.0.0.1\r\n"));
    }

    #[test]
    fn upgrade_request_does_not_forward_original_host() {
        let req = ws_request("/ws");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", Some(LOCALHOST));

        // Should have the backend Host, not the original
        assert!(raw.contains("Host: 127.0.0.1:3000\r\n"));
        // The original Host header should NOT appear as a repeated header.
        // Count lines starting with "Host:" (not substring matches like "X-Forwarded-Host:").
        let host_count = raw.lines().filter(|l| l.starts_with("Host:")).count();
        assert_eq!(host_count, 1);
    }

    #[test]
    fn upgrade_request_preserves_connection_and_upgrade() {
        let req = ws_request("/ws");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(raw.contains("upgrade: websocket\r\n"));
        assert!(raw.contains("connection: Upgrade\r\n"));
    }

    #[test]
    fn upgrade_request_strips_static_hop_by_hop_headers() {
        let req = Request::builder()
            .uri("/ws")
            .header(header::HOST, "myapp.roxy")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade")
            .header(header::PROXY_AUTHORIZATION, "Basic abc")
            .header(header::TE, "trailers")
            .header(header::TRAILER, "Expires")
            .header(header::TRANSFER_ENCODING, "chunked")
            .header(KEEP_ALIVE, "timeout=5")
            .body(Body::empty())
            .unwrap();
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(!raw.contains("proxy-authorization:"));
        assert!(!raw.contains("te:"));
        assert!(!raw.contains("trailer:"));
        assert!(!raw.contains("transfer-encoding:"));
        assert!(!raw.contains("keep-alive:"));
    }

    #[test]
    fn upgrade_request_strips_dynamic_hop_by_hop_from_connection() {
        let req = Request::builder()
            .uri("/ws")
            .header(header::HOST, "myapp.roxy")
            .header(header::UPGRADE, "websocket")
            .header(header::CONNECTION, "Upgrade, X-Secret")
            .header("x-secret", "leaked")
            .header("x-safe", "kept")
            .body(Body::empty())
            .unwrap();
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(!raw.contains("x-secret:"));
        assert!(raw.contains("x-safe: kept\r\n"));
    }

    #[test]
    fn upgrade_request_includes_query_string() {
        let req = ws_request("/ws?token=abc");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(raw.starts_with("GET /ws?token=abc HTTP/1.1\r\n"));
    }

    #[test]
    fn upgrade_request_ends_with_blank_line() {
        let req = ws_request("/ws");
        let target = make_target();

        let raw = build_upgrade_request(&req, &target, "myapp.roxy", "https", None);

        assert!(raw.ends_with("\r\n\r\n"));
    }

    #[test]
    fn render_proxy_connect_error_page_includes_diagnostics_and_target() {
        let target = ProxyTarget::parse("3000").unwrap();

        let html = render_proxy_error_page(
            "demo.localify",
            &target,
            ProxyErrorKind::Connect,
            "client error (Connect)",
            None,
        );

        assert!(html.contains("Service Unreachable"));
        assert!(html.contains("demo.localify"));
        assert!(html.contains("127.0.0.1:3000"));
        assert!(html.contains("lsof -nP -iTCP:3000 -sTCP:LISTEN"));
        assert!(html.contains("localify list"));
        assert!(html.contains("client error (Connect)"));
    }

    #[test]
    fn render_proxy_invalid_target_page_includes_route_fix_hint() {
        let target = ProxyTarget::parse("3000").unwrap();

        let html = render_proxy_error_page(
            "demo.localify",
            &target,
            ProxyErrorKind::InvalidTargetUri,
            "Invalid URI: bad authority",
            None,
        );

        assert!(html.contains("Invalid Route Target"));
        assert!(html.contains("localify route add demo.localify / 3000"));
        assert!(html.contains("Invalid URI: bad authority"));
    }

    #[test]
    fn render_proxy_error_page_shows_open_in_editor_when_project_root_is_available() {
        let target = ProxyTarget::parse("3000").unwrap();

        let html = render_proxy_error_page(
            "demo.localify",
            &target,
            ProxyErrorKind::Connect,
            "client error (Connect)",
            Some("/tmp/demo"),
        );

        assert!(html.contains("Open in editor"));
        assert!(html.contains("/.localify/open-in-editor"));
        assert!(html.contains("/tmp/demo"));
    }
}
