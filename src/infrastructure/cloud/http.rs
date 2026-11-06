use std::future::Future;
use std::time::Duration;

use anyhow::Context;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use hyper::{Method, Request, StatusCode};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde::Deserialize;
use serde::de::DeserializeOwned;

pub const DEFAULT_API_BASE: &str = "https://api.localify.dev";
/// Points the CLI, the app and the daemon at another API, such as
/// `wrangler dev` while working on the cloud.
const API_BASE_ENV: &str = "LOCALIFY_CLOUD_API";
/// For the whole exchange, body included.
const TIMEOUT: Duration = Duration::from_secs(20);
/// The API's largest answer is a list of at most 10 public URLs.
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The API base for a new sign-in. A signed-in machine uses the one saved in
/// its credentials.
pub fn api_base() -> String {
    std::env::var(API_BASE_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    /// The API refused the request and said why.
    #[error("{message}")]
    Api {
        status: u16,
        code: String,
        message: String,
        /// Seconds to wait before the next poll, with `slow_down`.
        interval: Option<u64>,
    },
    #[error(
        "Could not reach Localify Cloud at {api_base}: {reason}. Check the connection and try again."
    )]
    Unreachable { api_base: String, reason: String },
    #[error(
        "Localify Cloud sent a response this version cannot read ({0}). Update Localify and try again."
    )]
    InvalidResponse(String),
}

#[cfg(test)]
impl CloudError {
    /// The API's error code, such as `authorization_pending`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Api { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// A JSON client for the Localify Cloud API.
#[derive(Clone)]
pub struct CloudHttp {
    api_base: String,
    token: Option<String>,
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl CloudHttp {
    /// `token` is `None` only for the device flow, before sign-in.
    pub fn new(api_base: impl Into<String>, token: Option<String>) -> Result<Self, CloudError> {
        let api_base = api_base.into();
        check_api_base(&api_base)?;
        // An explicit provider, because the desktop app never installs a
        // process-wide default one.
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider())
            .map_err(|error| CloudError::Unreachable {
                api_base: api_base.clone(),
                reason: format!("TLS setup failed: {error}"),
            })?
            // `check_api_base` allows plain HTTP only for a loopback `wrangler dev`.
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            api_base,
            token,
            client: Client::builder(TokioExecutor::new()).build(connector),
        })
    }

    /// Sends a request and reads a JSON response.
    pub async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, CloudError> {
        let bytes = self.request(method, path, body).await?;
        serde_json::from_slice(&bytes)
            .map_err(|error| CloudError::InvalidResponse(error.to_string()))
    }

    /// Sends a request whose success has no body, such as a `204`.
    pub async fn send_no_content(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(), CloudError> {
        self.request(method, path, body).await.map(|_| ())
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Bytes, CloudError> {
        let request = self.build(method, path, body)?;
        let (status, bytes) = tokio::time::timeout(TIMEOUT, self.exchange(request))
            .await
            .map_err(|_| CloudError::Unreachable {
                api_base: self.api_base.clone(),
                reason: format!("no complete answer after {} s", TIMEOUT.as_secs()),
            })??;
        if status.is_success() {
            Ok(bytes)
        } else {
            Err(api_error(status, &bytes))
        }
    }

    async fn exchange(
        &self,
        request: Request<Full<Bytes>>,
    ) -> Result<(StatusCode, Bytes), CloudError> {
        let unreachable = |reason: String| CloudError::Unreachable {
            api_base: self.api_base.clone(),
            reason,
        };
        let response = self
            .client
            .request(request)
            .await
            .map_err(|error| unreachable(error_chain(&error)))?;
        let status = response.status();
        let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|error| unreachable(error_chain(error.as_ref())))?
            .to_bytes();
        Ok((status, bytes))
    }

    fn build(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Request<Full<Bytes>>, CloudError> {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.api_base))
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("localify/", env!("CARGO_PKG_VERSION")));
        if let Some(token) = &self.token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = match body {
            Some(value) => {
                builder = builder.header(CONTENT_TYPE, "application/json");
                Full::new(Bytes::from(value.to_string()))
            }
            None => Full::new(Bytes::new()),
        };
        builder.body(body).map_err(|error| CloudError::Unreachable {
            api_base: self.api_base.clone(),
            reason: format!("invalid request: {error}"),
        })
    }
}

/// A token must never cross the network in clear text: plain HTTP is only for
/// a `wrangler dev` on this machine.
fn check_api_base(api_base: &str) -> Result<(), CloudError> {
    let Some(rest) = api_base.strip_prefix("http://") else {
        return Ok(());
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    Err(CloudError::Unreachable {
        api_base: api_base.to_string(),
        reason: "plain http:// is only allowed for 127.0.0.1, localhost or [::1]".to_string(),
    })
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
    message: String,
    #[serde(default)]
    interval: Option<u64>,
}

fn api_error(status: StatusCode, body: &[u8]) -> CloudError {
    match serde_json::from_slice::<ErrorBody>(body) {
        Ok(body) => CloudError::Api {
            status: status.as_u16(),
            code: body.error,
            message: body.message,
            interval: body.interval,
        },
        Err(_) => CloudError::Api {
            status: status.as_u16(),
            code: format!("http_{}", status.as_u16()),
            message: format!("Localify Cloud answered {status}. Try again in a moment."),
            interval: None,
        },
    }
}

/// hyper's top-level errors say little ("client error (Connect)"); the cause
/// is further down the chain.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(": ")
}

/// Runs one cloud call from synchronous code, such as the CLI and
/// `DesktopApi`. It starts its own runtime, so async code must `.await` the
/// call instead.
pub fn block_on<F: Future>(future: F) -> anyhow::Result<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Could not start the runtime for a Localify Cloud call")?;
    Ok(runtime.block_on(future))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_error_keeps_its_code_and_message() {
        let error = api_error(
            StatusCode::CONFLICT,
            br#"{"error":"slug_taken","message":"journal-a8f3 is taken. Choose another slug."}"#,
        );
        assert_eq!(error.code(), Some("slug_taken"));
        assert_eq!(
            error.to_string(),
            "journal-a8f3 is taken. Choose another slug."
        );
    }

    #[test]
    fn slow_down_keeps_the_new_interval() {
        let error = api_error(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"error":"slow_down","message":"Slow down.","interval":10}"#,
        );
        assert!(matches!(
            error,
            CloudError::Api {
                interval: Some(10),
                ..
            }
        ));
    }

    #[test]
    fn an_unreadable_error_body_still_gives_a_sentence() {
        let error = api_error(StatusCode::BAD_GATEWAY, b"<html>");
        assert_eq!(error.code(), Some("http_502"));
        assert!(error.to_string().contains("502"));
    }

    #[test]
    fn plain_http_is_refused_except_on_loopback() {
        assert!(CloudHttp::new("http://127.0.0.1:8787", None).is_ok());
        assert!(CloudHttp::new("http://localhost:8787", None).is_ok());
        assert!(CloudHttp::new("http://[::1]:8787", None).is_ok());
        assert!(CloudHttp::new("https://api.localify.dev", None).is_ok());
        assert!(CloudHttp::new("http://api.localify.dev", None).is_err());
        assert!(CloudHttp::new("http://127.0.0.1.evil.example", None).is_err());
        assert!(CloudHttp::new("http://[2001:db8::1]:8787", None).is_err());
    }

    #[test]
    fn requests_carry_the_token_and_a_json_body() {
        let http = CloudHttp::new("https://api.example", Some("lfy_t".into())).unwrap();
        let request = http
            .build(
                Method::POST,
                "/v1/public-urls",
                Some(serde_json::json!({"localDomain": "journal.localify"})),
            )
            .unwrap();
        assert_eq!(request.uri(), "https://api.example/v1/public-urls");
        assert_eq!(request.headers()[AUTHORIZATION], "Bearer lfy_t");
        assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
    }
}
