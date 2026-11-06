//! Typed calls to the Localify Cloud API (`docs/public-urls.md`, section
//! "API"). The daemon's own route, `GET /v1/machines/self/runtime`, lives with
//! the daemon.

use std::fmt;

use hyper::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use super::http::{CloudError, CloudHttp};
use super::types::{CloudMachine, CloudUser, PublicUrl};

/// GitHub's device flow, started by Localify Cloud.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// A signed-in machine's token, and who it is.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceToken {
    pub token: String,
    pub user: CloudUser,
    pub machine: CloudMachine,
}

/// Leaves the token out, so a response can appear in logs and errors.
impl fmt::Debug for DeviceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceToken")
            .field("token", &"<redacted>")
            .field("user", &self.user)
            .field("machine", &self.machine)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub user: CloudUser,
    pub machine: CloudMachine,
}

/// A public URL, and the password Localify Cloud generated for it, if it did.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlWithPassword {
    pub public_url: PublicUrl,
    #[serde(default)]
    pub password: Option<String>,
}

/// The `protection` object of a request. A password of `None` asks Localify
/// Cloud to generate one.
#[derive(Clone, PartialEq, Eq)]
pub enum ProtectionRequest {
    Password(Option<String>),
    Public,
}

impl ProtectionRequest {
    fn body(&self) -> Value {
        match self {
            Self::Password(Some(password)) => json!({ "mode": "password", "password": password }),
            Self::Password(None) => json!({ "mode": "password" }),
            Self::Public => json!({ "mode": "public" }),
        }
    }
}

pub struct CreatePublicUrlRequest<'a> {
    pub local_domain: &'a str,
    pub wildcard: bool,
    /// Localify Cloud generates one when `None`.
    pub slug: Option<&'a str>,
    pub protection: &'a ProtectionRequest,
}

impl CreatePublicUrlRequest<'_> {
    fn body(&self) -> Value {
        let mut body = json!({
            "localDomain": self.local_domain,
            "wildcard": self.wildcard,
            "protection": self.protection.body(),
        });
        if let Some(slug) = self.slug {
            body["slug"] = json!(slug);
        }
        body
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicUrlList {
    public_urls: Vec<PublicUrl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicUrlBody {
    public_url: PublicUrl,
}

/// The slug a new URL would get, and its URL.
#[allow(dead_code)] // Only the app asks; the CLI target does not build the app.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlugSuggestion {
    pub slug: String,
    pub url: String,
}

/// Whether a slug is free for the caller.
#[allow(dead_code)] // Only the app asks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlugAvailability {
    /// Lower-cased by the API.
    pub slug: String,
    pub available: bool,
}

/// The routes of the Localify Cloud API, for the CLI and the app.
pub struct CloudClient {
    http: CloudHttp,
}

impl CloudClient {
    /// `http` carries the token, except for the device flow.
    pub fn new(http: CloudHttp) -> Self {
        Self { http }
    }

    pub async fn start_device_flow(&self) -> Result<DeviceCode, CloudError> {
        self.http.send(Method::POST, "/v1/auth/device", None).await
    }

    /// Fails with the codes `authorization_pending`, `slow_down`,
    /// `not_allowed` and `expired_token` until the person confirms the code.
    /// On success Localify Cloud registers this machine and its tunnel.
    pub async fn poll_device_token(
        &self,
        device_code: &str,
        machine_name: &str,
        ingress_port: u16,
    ) -> Result<DeviceToken, CloudError> {
        let body = json!({
            "deviceCode": device_code,
            "machineName": machine_name,
            "ingressPort": ingress_port,
        });
        self.http
            .send(Method::POST, "/v1/auth/device/token", Some(body))
            .await
    }

    #[allow(dead_code)] // No caller yet: `whoami` reads the saved credentials offline.
    pub async fn me(&self) -> Result<Me, CloudError> {
        self.http.send(Method::GET, "/v1/me", None).await
    }

    /// Revokes the token, deletes the machine's tunnel and detaches its URLs.
    pub async fn logout(&self) -> Result<(), CloudError> {
        self.http
            .send_no_content(Method::POST, "/v1/auth/logout", None)
            .await
    }

    /// The first free slug that mirrors the local domain's name.
    #[allow(dead_code)] // Only the app asks.
    pub async fn suggest_slug(&self, local_domain: &str) -> Result<SlugSuggestion, CloudError> {
        let path = format!("/v1/slugs?localDomain={}", encode_component(local_domain));
        self.http.send(Method::GET, &path, None).await
    }

    #[allow(dead_code)] // Only the app asks.
    pub async fn slug_availability(&self, slug: &str) -> Result<SlugAvailability, CloudError> {
        let path = format!("/v1/slugs/{}", encode_component(slug));
        self.http.send(Method::GET, &path, None).await
    }

    pub async fn list_public_urls(&self) -> Result<Vec<PublicUrl>, CloudError> {
        let list: PublicUrlList = self.http.send(Method::GET, "/v1/public-urls", None).await?;
        Ok(list.public_urls)
    }

    /// The calling machine serves the new URL.
    pub async fn create_public_url(
        &self,
        request: &CreatePublicUrlRequest<'_>,
    ) -> Result<PublicUrlWithPassword, CloudError> {
        self.http
            .send(Method::POST, "/v1/public-urls", Some(request.body()))
            .await
    }

    pub async fn delete_public_url(&self, id: &str) -> Result<(), CloudError> {
        self.http
            .send_no_content(Method::DELETE, &url_path(id, ""), None)
            .await
    }

    /// Changing or removing the password signs every browser out.
    pub async fn set_protection(
        &self,
        id: &str,
        protection: &ProtectionRequest,
    ) -> Result<PublicUrlWithPassword, CloudError> {
        self.http
            .send(
                Method::PUT,
                &url_path(id, "/protection"),
                Some(serde_json::json!({ "protection": protection.body() })),
            )
            .await
    }

    /// Moves the URL to the calling machine.
    pub async fn claim(&self, id: &str) -> Result<PublicUrl, CloudError> {
        let body: PublicUrlBody = self
            .http
            .send(Method::POST, &url_path(id, "/claim"), None)
            .await?;
        Ok(body.public_url)
    }
}

/// `/v1/public-urls/<id><suffix>`, with the id encoded as one path segment.
fn url_path(id: &str, suffix: &str) -> String {
    format!("/v1/public-urls/{}{suffix}", encode_component(id))
}

/// Encodes a value as one path segment or query value.
fn encode_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use crate::infrastructure::cloud::http::block_on;
    use crate::infrastructure::cloud::types::ProtectionMode;

    const URL_JSON: &str = r#"{"id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
        "localDomain":"journal.localify","wildcard":false,"machine":null,
        "protection":{"mode":"password"},"createdAt":"2026-11-05T18:00:00Z"}"#;

    /// Answers one request with `status` and `body`, and returns the request
    /// line, the headers and the body it received.
    fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<(String, String)>) {
        // IPv6 loopback, so it never takes a 127.0.0.1 port that another
        // test just freed and is probing.
        let listener = TcpListener::bind("[::1]:0")
            .or_else(|_| TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            reader.get_mut().write_all(response.as_bytes()).unwrap();
            (head, String::from_utf8(body).unwrap())
        });
        (address, handle)
    }

    fn client(address: &str) -> CloudClient {
        CloudClient::new(CloudHttp::new(address, Some("lfy_t".into())).unwrap())
    }

    #[test]
    fn rotating_a_password_puts_the_protection_with_the_token() {
        let (address, server) = serve_once("200 OK", &format!(r#"{{"publicUrl":{URL_JSON}}}"#));

        let changed = block_on(
            client(&address).set_protection("u1", &ProtectionRequest::Password(Some("p".into()))),
        )
        .unwrap()
        .unwrap();

        let (head, body) = server.join().unwrap();
        assert!(
            head.starts_with("PUT /v1/public-urls/u1/protection HTTP/1.1"),
            "{head}"
        );
        assert!(
            head.to_ascii_lowercase()
                .contains("authorization: bearer lfy_t")
        );
        assert_eq!(body, r#"{"protection":{"mode":"password","password":"p"}}"#);
        assert_eq!(changed.public_url.protection.mode, ProtectionMode::Password);
        assert_eq!(changed.password, None);
    }

    #[test]
    fn removing_a_url_expects_no_content() {
        let (address, server) = serve_once("204 No Content", "");

        block_on(client(&address).delete_public_url("u1"))
            .unwrap()
            .unwrap();

        let (head, _) = server.join().unwrap();
        assert!(
            head.starts_with("DELETE /v1/public-urls/u1 HTTP/1.1"),
            "{head}"
        );
    }

    #[test]
    fn a_created_url_carries_the_generated_password() {
        let (address, server) = serve_once(
            "201 Created",
            &format!(r#"{{"publicUrl":{URL_JSON},"password":"tide-maple-orbit-quill"}}"#),
        );
        let protection = ProtectionRequest::Password(None);

        let created = block_on(client(&address).create_public_url(&CreatePublicUrlRequest {
            local_domain: "journal.localify",
            wildcard: false,
            slug: None,
            protection: &protection,
        }))
        .unwrap()
        .unwrap();

        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /v1/public-urls HTTP/1.1"), "{head}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            body,
            json!({"localDomain": "journal.localify", "wildcard": false, "protection": {"mode": "password"}})
        );
        assert_eq!(created.password.as_deref(), Some("tide-maple-orbit-quill"));
    }

    #[test]
    fn a_pending_sign_in_is_an_error_with_its_code() {
        let (address, server) = serve_once(
            "428 Precondition Required",
            r#"{"error":"authorization_pending","message":"Confirm the code on github.com."}"#,
        );
        let client = CloudClient::new(CloudHttp::new(&address, None).unwrap());

        let error = block_on(client.poll_device_token("dc", "laptop", 7381))
            .unwrap()
            .unwrap_err();

        let (head, body) = server.join().unwrap();
        assert!(!head.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap(),
            json!({"deviceCode": "dc", "machineName": "laptop", "ingressPort": 7381})
        );
        assert_eq!(error.code(), Some("authorization_pending"));
    }

    #[test]
    fn protection_bodies_follow_the_contract() {
        assert_eq!(
            ProtectionRequest::Password(Some("p".into())).body(),
            json!({"mode": "password", "password": "p"})
        );
        assert_eq!(ProtectionRequest::Public.body(), json!({"mode": "public"}));
        let request = CreatePublicUrlRequest {
            local_domain: "shop.localify",
            wildcard: true,
            slug: Some("shop"),
            protection: &ProtectionRequest::Public,
        };
        assert_eq!(request.body()["slug"], "shop");
    }

    #[test]
    fn a_suggestion_asks_for_the_local_domain() {
        let (address, server) = serve_once(
            "200 OK",
            r#"{"slug":"louez","url":"https://louez.localify.fyi"}"#,
        );

        let suggestion = block_on(client(&address).suggest_slug("web.louez.localify"))
            .unwrap()
            .unwrap();

        let (head, _) = server.join().unwrap();
        assert!(
            head.starts_with("GET /v1/slugs?localDomain=web.louez.localify HTTP/1.1"),
            "{head}"
        );
        assert_eq!(
            suggestion,
            SlugSuggestion {
                slug: "louez".into(),
                url: "https://louez.localify.fyi".into(),
            }
        );
    }

    #[test]
    fn availability_reads_the_slug_and_its_answer() {
        let (address, server) = serve_once("200 OK", r#"{"slug":"louez","available":false}"#);

        let answer = block_on(client(&address).slug_availability("louez"))
            .unwrap()
            .unwrap();

        let (head, _) = server.join().unwrap();
        assert!(head.starts_with("GET /v1/slugs/louez HTTP/1.1"), "{head}");
        assert_eq!(
            answer,
            SlugAvailability {
                slug: "louez".into(),
                available: false
            }
        );
    }

    #[test]
    fn an_id_stays_one_path_segment() {
        assert_eq!(url_path("u1", "/claim"), "/v1/public-urls/u1/claim");
        assert_eq!(url_path("a/b?c", ""), "/v1/public-urls/a%2Fb%3Fc");
    }

    #[test]
    fn a_device_token_never_shows_its_token() {
        let token: DeviceToken = serde_json::from_str(
            r#"{"token":"lfy_secret","user":{"login":"teo"},"machine":{"id":"m1","name":"laptop"}}"#,
        )
        .unwrap();
        assert!(!format!("{token:?}").contains("lfy_secret"));
    }
}
