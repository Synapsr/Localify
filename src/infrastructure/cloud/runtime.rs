//! `GET /v2/machines/self/runtime`, and `/v1` for a cloud that has no `/v2`
//! yet (`docs/public-urls.md`, sections "API" and "Runtime").

use std::fmt;
use std::time::Duration;

use hyper::Method;

use super::http::{CloudError, CloudHttp};
use super::types::{PublicUrl, RuntimeBlock, RuntimeV1, RuntimeV2, RuntimeV2State};

const RUNTIME_V2_PATH: &str = "/v2/machines/self/runtime";
const RUNTIME_V1_PATH: &str = "/v1/machines/self/runtime";
/// What `/v1` stands for: it has no generations, and 0 names no tunnel.
pub const V1_GENERATION: u64 = 0;

/// What Localify Cloud wants this machine to do with its tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runtime {
    /// Nothing to serve.
    Idle,
    /// A tunnel is on its way: ask again after `retry_after`.
    Provisioning {
        retry_after: Option<Duration>,
    },
    Ready(ReadyRuntime),
    /// Something to serve, but no right to: final until a plan or URL change,
    /// or until `retry_after` when it is set (the account cap).
    Blocked {
        block: RuntimeBlock,
        retry_after: Option<Duration>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReadyRuntime {
    pub generation: u64,
    /// Only when `generation` differs from the one the request named.
    pub token: Option<String>,
    pub ingress_secret: String,
    pub public_urls: Vec<PublicUrl>,
}

/// Leaves both secrets out.
impl fmt::Debug for ReadyRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadyRuntime")
            .field("generation", &self.generation)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("ingress_secret", &"<redacted>")
            .field("public_urls", &self.public_urls)
            .finish()
    }
}

/// Asks what to serve, naming the tunnel `generation` this machine runs, or
/// 0 for none. A cloud that answers 404 on `/v2` is older than it: `/v1`
/// answers instead, as a `Ready` runtime at generation 0 with its token every
/// time. A revoked sign-in answers `CloudError::Api` with status 401.
pub async fn fetch_runtime(http: &CloudHttp, generation: u64) -> Result<Runtime, CloudError> {
    let path = format!("{RUNTIME_V2_PATH}?generation={generation}");
    match http.send::<RuntimeV2>(Method::GET, &path, None).await {
        Ok(runtime) => from_v2(runtime),
        Err(CloudError::Api { status: 404, .. }) => {
            let runtime: RuntimeV1 = http.send(Method::GET, RUNTIME_V1_PATH, None).await?;
            Ok(Runtime::Ready(ReadyRuntime {
                generation: V1_GENERATION,
                token: Some(runtime.tunnel_token),
                ingress_secret: runtime.ingress_secret,
                public_urls: runtime.public_urls,
            }))
        }
        Err(error) => Err(error),
    }
}

/// Checks what each state needs, so the daemon never acts on half an answer.
fn from_v2(runtime: RuntimeV2) -> Result<Runtime, CloudError> {
    let retry_after = runtime.retry_after.map(Duration::from_secs);
    let missing = |what: &str| CloudError::InvalidResponse(format!("a runtime without {what}"));
    Ok(match runtime.state {
        RuntimeV2State::Idle => Runtime::Idle,
        RuntimeV2State::Provisioning => Runtime::Provisioning { retry_after },
        RuntimeV2State::Blocked => Runtime::Blocked {
            block: runtime.blocked.ok_or_else(|| missing("its reason"))?,
            retry_after,
        },
        RuntimeV2State::Ready => {
            let tunnel = runtime.tunnel.ok_or_else(|| missing("its tunnel"))?;
            Runtime::Ready(ReadyRuntime {
                generation: tunnel.generation,
                token: tunnel.token,
                ingress_secret: runtime
                    .ingress_secret
                    .ok_or_else(|| missing("its ingress secret"))?,
                public_urls: runtime.public_urls,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const URL: &str = r#"{"id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
        "localDomain":"journal.localify","wildcard":false,
        "machine":{"id":"m1","name":"laptop"},
        "protection":{"mode":"public"},"state":"serving",
        "createdAt":"2026-11-05T18:00:00Z"}"#;

    /// Answers one request per `(status, body)`, in order, and hands back the
    /// requests it read.
    async fn fake_api(
        answers: Vec<(&'static str, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                requests.push(String::from_utf8_lossy(&buffer[..read]).to_string());
            }
            requests
        });
        (base, server)
    }

    fn http(base: &str) -> CloudHttp {
        CloudHttp::new(base, Some("lfy_t".into())).unwrap()
    }

    #[tokio::test]
    async fn a_ready_runtime_names_the_generation_with_the_machine_token() {
        let body = format!(
            r#"{{"state":"ready","blocked":null,"tunnel":{{"generation":3,"token":"tok-3"}},
                "ingressSecret":"abc123","publicUrls":[{URL}],"retryAfter":null}}"#
        );
        let (base, server) = fake_api(vec![("200 OK", body)]).await;

        let runtime = fetch_runtime(&http(&base), 2).await.unwrap();

        let Runtime::Ready(ready) = runtime else {
            panic!("expected ready, got {runtime:?}");
        };
        assert_eq!(ready.generation, 3);
        assert_eq!(ready.token.as_deref(), Some("tok-3"));
        assert_eq!(ready.ingress_secret, "abc123");
        assert_eq!(ready.public_urls[0].id, "u1");
        let requests = server.await.unwrap();
        assert!(
            requests[0].starts_with("GET /v2/machines/self/runtime?generation=2 HTTP/1.1\r\n"),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("authorization: Bearer lfy_t\r\n"));
    }

    #[tokio::test]
    async fn every_other_state_reads_its_fields() {
        let blocked = r#"{"state":"blocked","blocked":{"reason":"capacity","message":"Full."},
            "tunnel":null,"ingressSecret":null,"publicUrls":[],"retryAfter":60}"#;
        let provisioning = r#"{"state":"provisioning","blocked":null,"tunnel":null,
            "ingressSecret":null,"publicUrls":[],"retryAfter":3}"#;
        let idle = r#"{"state":"idle","blocked":null,"tunnel":null,"ingressSecret":null,
            "publicUrls":[],"retryAfter":null}"#;
        let (base, _server) = fake_api(vec![
            ("200 OK", blocked.into()),
            ("200 OK", provisioning.into()),
            ("200 OK", idle.into()),
        ])
        .await;
        let http = http(&base);

        assert_eq!(
            fetch_runtime(&http, 0).await.unwrap(),
            Runtime::Blocked {
                block: RuntimeBlock {
                    reason: "capacity".into(),
                    message: "Full.".into(),
                    upgrade: None,
                },
                retry_after: Some(Duration::from_secs(60)),
            }
        );
        assert_eq!(
            fetch_runtime(&http, 0).await.unwrap(),
            Runtime::Provisioning {
                retry_after: Some(Duration::from_secs(3))
            }
        );
        assert_eq!(fetch_runtime(&http, 0).await.unwrap(), Runtime::Idle);
    }

    #[tokio::test]
    async fn a_cloud_without_v2_answers_through_v1() {
        let v1 =
            format!(r#"{{"tunnelToken":"tok","ingressSecret":"abc123","publicUrls":[{URL}]}}"#);
        let (base, server) = fake_api(vec![
            (
                "404 Not Found",
                r#"{"error":"not_found","message":"Not found."}"#.into(),
            ),
            ("200 OK", v1),
        ])
        .await;

        let runtime = fetch_runtime(&http(&base), 4).await.unwrap();

        let Runtime::Ready(ready) = runtime else {
            panic!("expected ready, got {runtime:?}");
        };
        assert_eq!(ready.generation, V1_GENERATION);
        assert_eq!(ready.token.as_deref(), Some("tok"));
        let requests = server.await.unwrap();
        assert!(requests[1].starts_with("GET /v1/machines/self/runtime HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn a_revoked_sign_in_answers_401_without_trying_v1() {
        let (base, server) = fake_api(vec![(
            "401 Unauthorized",
            r#"{"error":"unauthorized","message":"Sign in again."}"#.into(),
        )])
        .await;

        let error = fetch_runtime(&http(&base), 0).await.unwrap_err();

        assert!(matches!(error, CloudError::Api { status: 401, .. }));
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn half_an_answer_is_refused() {
        let ready_without_secret = r#"{"state":"ready","tunnel":{"generation":1,"token":"t"}}"#;
        let blocked_without_reason = r#"{"state":"blocked"}"#;
        let v1_without_secret = r#"{"tunnelToken":"tok","publicUrls":[]}"#;
        let (base, _server) = fake_api(vec![
            ("200 OK", ready_without_secret.into()),
            ("200 OK", blocked_without_reason.into()),
            ("404 Not Found", "{}".into()),
            ("200 OK", v1_without_secret.into()),
        ])
        .await;
        let http = http(&base);

        for _ in 0..3 {
            let error = fetch_runtime(&http, 0).await.unwrap_err();
            assert!(matches!(error, CloudError::InvalidResponse(_)), "{error:?}");
        }
    }
}
