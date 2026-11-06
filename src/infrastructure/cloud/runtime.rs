//! `GET /v1/machines/self/runtime` (`docs/public-urls.md`, section "API").

use hyper::Method;

use super::http::{CloudError, CloudHttp};
use super::types::MachineRuntime;

const RUNTIME_PATH: &str = "/v1/machines/self/runtime";

/// The tunnel token, the ingress secret and the public URLs this machine
/// serves. The daemon asks at start, every few minutes and on
/// `public_refresh`, so a URL created or claimed from anywhere reaches its
/// tunnel. A revoked sign-in answers `CloudError::Api` with status 401.
pub async fn fetch_runtime(http: &CloudHttp) -> Result<MachineRuntime, CloudError> {
    http.send(Method::GET, RUNTIME_PATH, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Answers one request with `status` and `body`, and hands back what it read.
    async fn fake_api(
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 4096];
            let read = stream.read(&mut buffer).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buffer[..read]).to_string()
        });
        (base, server)
    }

    #[tokio::test]
    async fn the_runtime_reads_the_api_shape_with_the_machine_token() {
        let (base, server) = fake_api(
            "200 OK",
            r#"{"tunnelToken":"tok","ingressSecret":"abc123","publicUrls":[{
                "id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
                "localDomain":"journal.localify","wildcard":false,
                "machine":{"id":"m1","name":"laptop"},
                "protection":{"mode":"public","exceptions":[]},
                "createdAt":"2026-11-05T18:00:00Z"}]}"#,
        )
        .await;
        let http = CloudHttp::new(base, Some("lfy_t".into())).unwrap();

        let runtime = fetch_runtime(&http).await.unwrap();

        assert_eq!(runtime.tunnel_token, "tok");
        assert_eq!(runtime.ingress_secret, "abc123");
        assert_eq!(runtime.public_urls[0].id, "u1");
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /v1/machines/self/runtime HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer lfy_t\r\n"));
    }

    #[tokio::test]
    async fn a_revoked_sign_in_answers_401() {
        let (base, _server) = fake_api(
            "401 Unauthorized",
            r#"{"error":"unauthorized","message":"Sign in again."}"#,
        )
        .await;
        let http = CloudHttp::new(base, Some("lfy_old".into())).unwrap();

        let error = fetch_runtime(&http).await.unwrap_err();

        assert!(matches!(error, CloudError::Api { status: 401, .. }));
    }

    #[tokio::test]
    async fn a_runtime_without_its_secret_is_refused() {
        let (base, _server) = fake_api("200 OK", r#"{"tunnelToken":"tok","publicUrls":[]}"#).await;
        let http = CloudHttp::new(base, Some("lfy_t".into())).unwrap();

        let error = fetch_runtime(&http).await.unwrap_err();

        assert!(matches!(error, CloudError::InvalidResponse(_)));
    }
}
