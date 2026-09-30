//! Browser indicator for services running on a dangerous launch profile.
//!
//! The proxy adds one `<script src>` tag right after `<head>` in HTML page
//! navigations. The script is served from the app's own origin, so a CSP
//! allowing `'self'` scripts runs it; when the page's CSP uses a nonce, the
//! tag reuses it. The script draws its pill through the CSSOM in a shadow
//! root, which `style-src` does not restrict, and only touches the page after
//! `load`, once frameworks have hydrated.
//!
//! Only the start of the document is scanned; the rest of the stream passes
//! through untouched, so streamed HTML keeps streaming.

use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;

/// Reserved path on every proxied domain, next to `/.localify/open-in-editor`.
pub const SCRIPT_PATH: &str = "/.localify/launch-profile.js";

const SCRIPT: &str = include_str!("templates/launch_profile_indicator.js");

/// Stop looking for `<head>` after this many bytes and pass the page through.
const SCAN_LIMIT: usize = 64 * 1024;

pub fn script_response() -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        SCRIPT,
    )
        .into_response()
}

/// Top-level page navigations only: not fetches, iframes, assets or HMR.
pub fn is_page_navigation(request: &Request) -> bool {
    if request.method() != Method::GET {
        return false;
    }
    let headers = request.headers();
    match header_str(headers, "sec-fetch-dest") {
        Some(destination) => destination == "document",
        None => header_str(headers, header::ACCEPT.as_str())
            .is_some_and(|accept| accept.contains("text/html")),
    }
}

/// Asks the upstream for an uncompressed page so the tag can be added.
pub fn request_identity_encoding(request: &mut Request) {
    request.headers_mut().remove(header::ACCEPT_ENCODING);
}

/// Adds the indicator script to an HTML page response. Anything else, and
/// compressed pages the upstream sent anyway, pass through unchanged.
pub fn inject(response: Response, profile: &str) -> Response {
    let headers = response.headers();
    let is_html = header_str(headers, header::CONTENT_TYPE.as_str())
        .is_some_and(|content_type| content_type.starts_with("text/html"));
    if !is_html || headers.contains_key(header::CONTENT_ENCODING) {
        return response;
    }

    let tag = script_tag(profile, csp_nonce(headers).as_deref());
    let (mut parts, body) = response.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, inject_into_body(body, tag))
}

fn script_tag(profile: &str, nonce: Option<&str>) -> String {
    let nonce = nonce
        .map(|nonce| format!(" nonce=\"{nonce}\""))
        .unwrap_or_default();
    // Launch profile names are restricted to [a-z0-9_-]; nonces are base64.
    format!("<script src=\"{SCRIPT_PATH}\" data-profile=\"{profile}\"{nonce} defer></script>")
}

/// Nonce the page's CSP header already allows for scripts, if any.
fn csp_nonce(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|policy| policy.split([' ', ';']))
        .find_map(|token| {
            token
                .strip_prefix("'nonce-")
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .filter(|nonce| {
            nonce
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_'))
        })
        .map(ToOwned::to_owned)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value: &HeaderValue| value.to_str().ok())
}

fn inject_into_body(body: Body, tag: String) -> Body {
    let chunks = body.into_data_stream();
    let stream = futures_util::stream::unfold(
        (chunks, HeadInjector::new(tag), false),
        |(mut chunks, mut injector, finished)| async move {
            if finished {
                return None;
            }
            match chunks.next().await {
                Some(Ok(chunk)) => {
                    let output = injector.push(&chunk);
                    Some((Ok(output), (chunks, injector, false)))
                }
                Some(Err(error)) => Some((Err(error), (chunks, injector, true))),
                None => {
                    let output = injector.finish();
                    Some((Ok(output), (chunks, injector, true)))
                }
            }
        },
    );
    Body::from_stream(stream)
}

/// Inserts a tag right after the opening `<head ...>` tag. Holds bytes back
/// only until that tag is found or `SCAN_LIMIT` is reached.
struct HeadInjector {
    tag: String,
    pending: Vec<u8>,
    done: bool,
}

impl HeadInjector {
    fn new(tag: String) -> Self {
        Self {
            tag,
            pending: Vec::new(),
            done: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Bytes {
        if self.done {
            return Bytes::copy_from_slice(chunk);
        }
        self.pending.extend_from_slice(chunk);

        if let Some(end) = head_tag_end(&self.pending) {
            let mut output = Vec::with_capacity(self.pending.len() + self.tag.len());
            output.extend_from_slice(&self.pending[..end]);
            output.extend_from_slice(self.tag.as_bytes());
            output.extend_from_slice(&self.pending[end..]);
            return self.release(output);
        }
        if self.pending.len() >= SCAN_LIMIT {
            let output = std::mem::take(&mut self.pending);
            return self.release(output);
        }
        Bytes::new()
    }

    fn finish(&mut self) -> Bytes {
        let output = std::mem::take(&mut self.pending);
        self.release(output)
    }

    fn release(&mut self, output: Vec<u8>) -> Bytes {
        self.done = true;
        self.pending.clear();
        Bytes::from(output)
    }
}

/// Byte offset just after the `>` of the first `<head>` or `<head ...>` tag.
fn head_tag_end(html: &[u8]) -> Option<usize> {
    let lowercase = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(offset) = find(&lowercase[from..], b"<head") {
        let start = from + offset;
        let after_name = start + b"<head".len();
        match lowercase.get(after_name) {
            Some(b'>') => return Some(after_name + 1),
            Some(c) if c.is_ascii_whitespace() => {
                return find(&lowercase[after_name..], b">").map(|end| after_name + end + 1);
            }
            // `<header>` or a tag split across chunks: keep looking.
            Some(_) => from = after_name,
            None => return None,
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injector() -> HeadInjector {
        HeadInjector::new("<script></script>".into())
    }

    fn collect(injector: &mut HeadInjector, chunks: &[&str]) -> String {
        let mut output = Vec::new();
        for chunk in chunks {
            output.extend_from_slice(&injector.push(chunk.as_bytes()));
        }
        output.extend_from_slice(&injector.finish());
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn injects_right_after_the_head_tag() {
        let html = collect(
            &mut injector(),
            &["<!DOCTYPE html><html><head><title>x</title></head><body></body></html>"],
        );

        assert_eq!(
            html,
            "<!DOCTYPE html><html><head><script></script><title>x</title></head><body></body></html>"
        );
    }

    #[test]
    fn handles_attributes_uppercase_and_split_chunks() {
        let html = collect(
            &mut injector(),
            &[
                "<html><HE",
                "AD data-x=\"1\"",
                "><title>x</title>",
                "</head>",
            ],
        );

        assert_eq!(
            html,
            "<html><HEAD data-x=\"1\"><script></script><title>x</title></head>"
        );
    }

    #[test]
    fn ignores_header_elements_and_passes_through_pages_without_head() {
        assert_eq!(
            collect(&mut injector(), &["<header>top</header><head>"]),
            "<header>top</header><head><script></script>"
        );
        assert_eq!(
            collect(&mut injector(), &["<p>fragment</p>"]),
            "<p>fragment</p>"
        );
    }

    #[test]
    fn stops_scanning_after_the_limit() {
        let mut injector = injector();
        let big = "a".repeat(SCAN_LIMIT);

        let first = injector.push(big.as_bytes());
        let second = injector.push(b"<head>");

        assert_eq!(first.len(), SCAN_LIMIT);
        assert_eq!(&second[..], b"<head>");
    }

    #[test]
    fn reuses_the_csp_script_nonce() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'nonce-abc123==' 'strict-dynamic'",
            ),
        );

        assert_eq!(csp_nonce(&headers).as_deref(), Some("abc123=="));
        assert_eq!(
            script_tag("prod", Some("abc123==")),
            "<script src=\"/.localify/launch-profile.js\" data-profile=\"prod\" nonce=\"abc123==\" defer></script>"
        );
        assert_eq!(csp_nonce(&HeaderMap::new()), None);
    }

    #[test]
    fn only_page_navigations_get_the_indicator() {
        let navigation = Request::builder()
            .uri("/")
            .header("sec-fetch-dest", "document")
            .body(Body::empty())
            .unwrap();
        let fetch = Request::builder()
            .uri("/api")
            .header("sec-fetch-dest", "empty")
            .header(header::ACCEPT, "text/html")
            .body(Body::empty())
            .unwrap();
        let legacy = Request::builder()
            .uri("/")
            .header(header::ACCEPT, "text/html,application/xhtml+xml")
            .body(Body::empty())
            .unwrap();
        let post = Request::builder()
            .method(Method::POST)
            .uri("/")
            .header("sec-fetch-dest", "document")
            .body(Body::empty())
            .unwrap();

        assert!(is_page_navigation(&navigation));
        assert!(!is_page_navigation(&fetch));
        assert!(is_page_navigation(&legacy));
        assert!(!is_page_navigation(&post));
    }

    #[tokio::test]
    async fn injects_into_html_responses_only() {
        let html = Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CONTENT_LENGTH, "13")
            .body(Body::from("<head></head>"))
            .unwrap();
        let json = Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{\"head\":1}"))
            .unwrap();

        let html = inject(html, "prod");
        assert!(html.headers().get(header::CONTENT_LENGTH).is_none());
        let body = axum::body::to_bytes(html.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("data-profile=\"prod\""));

        let json = inject(json, "prod");
        let body = axum::body::to_bytes(json.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"{\"head\":1}");
    }
}
