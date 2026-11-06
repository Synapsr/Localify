//! The loopback listener `cloudflared` delivers public traffic to
//! (`docs/public-urls.md`, section "Public ingress"). A request gets through
//! only when the Worker's secret and this machine's runtime inventory both
//! vouch for it; it then goes to the same router as local traffic.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::proxy::{PublicSource, public_error_response};
use super::router::{AdmittedState, RuntimeState, SharedState, create_router, is_localify_path};
use crate::infrastructure::cloud::types::PublicUrl;

const INGRESS_HEADER: &str = "x-localify-ingress";
const PUBLIC_HOST_HEADER: &str = "x-localify-public-host";
const LOCAL_HOST_HEADER: &str = "x-localify-local-host";
const URL_ID_HEADER: &str = "x-localify-url-id";
const CLIENT_IP_HEADER: &str = "x-localify-client-ip";
const LOCALIFY_HEADER_PREFIX: &str = "x-localify-";
/// Public requests waiting for their response head at once. Over HTTP/2 a
/// browser sends a whole module graph together, and the proxy opens one
/// upstream connection per request: without a cap the dev server's accept
/// backlog overflows.
pub const MAX_IN_FLIGHT: usize = 32;
/// `<slug>--<label>` is one DNS label.
const MAX_DNS_LABEL: usize = 63;

/// What the ingress accepts while this machine serves public URLs: the
/// Worker's secret, kept as its SHA-256, and the URLs served here.
pub struct IngressGrant {
    secret_digest: [u8; 32],
    urls: Vec<ServedUrl>,
}

impl IngressGrant {
    pub fn new(secret: &str, urls: &[PublicUrl]) -> Self {
        Self {
            secret_digest: Sha256::digest(secret.as_bytes()).into(),
            urls: urls.iter().filter_map(ServedUrl::from_api).collect(),
        }
    }

    /// Compares digests, so the time taken depends neither on where the
    /// values differ nor on the secret's length.
    fn accepts(&self, presented: Option<&HeaderValue>) -> bool {
        let Some(presented) = presented else {
            return false;
        };
        let digest: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        let difference = digest
            .iter()
            .zip(self.secret_digest.iter())
            .fold(0u8, |acc, (left, right)| acc | (left ^ right));
        difference == 0
    }
}

/// The current grant, shared between the tunnel supervisor, which grants and
/// revokes it, and the ingress, which checks every request against it.
#[derive(Clone, Default)]
pub struct IngressAccess(Arc<ArcSwapOption<IngressGrant>>);

impl IngressAccess {
    pub fn grant(&self, grant: IngressGrant) {
        self.0.store(Some(Arc::new(grant)));
    }

    /// From now on every public request gets 403.
    pub fn revoke(&self) {
        self.0.store(None);
    }

    fn current(&self) -> Option<Arc<IngressGrant>> {
        self.0.load_full()
    }

    /// Whether a request carrying `secret` would pass step 1.
    #[cfg(test)]
    pub fn accepts_secret(&self, secret: &str) -> bool {
        let presented = HeaderValue::from_str(secret).ok();
        self.current()
            .is_some_and(|grant| grant.accepts(presented.as_ref()))
    }
}

/// A public URL this machine serves, as the ingress matches requests to it.
struct ServedUrl {
    id: String,
    slug: String,
    /// `<slug>.localify.fyi`.
    host: String,
    /// `localify.fyi`.
    base_domain: String,
    local_domain: String,
    wildcard: bool,
}

impl ServedUrl {
    /// `None` for a URL whose address does not start with its slug, which a
    /// sound API never sends: such a URL matches no request.
    fn from_api(url: &PublicUrl) -> Option<Self> {
        let host = url
            .url
            .strip_prefix("https://")?
            .split('/')
            .next()?
            .to_ascii_lowercase();
        let slug = url.slug.to_ascii_lowercase();
        let base_domain = host.strip_prefix(&format!("{slug}."))?.to_string();
        Some(Self {
            id: url.id.clone(),
            slug,
            host,
            base_domain,
            local_domain: url.local_domain.to_ascii_lowercase(),
            wildcard: url.wildcard,
        })
    }

    /// The local host a public host of this URL maps to: `localDomain` for
    /// its own host, `<label>.<localDomain>` for `<slug>--<label>` on a
    /// wildcard URL (`docs/public-urls.md`, section "Names").
    fn local_host_for(&self, public_host: &str) -> Option<String> {
        if public_host == self.host {
            return Some(self.local_domain.clone());
        }
        if !self.wildcard {
            return None;
        }
        let first_label = public_host.strip_suffix(&format!(".{}", self.base_domain))?;
        let label = first_label.strip_prefix(&format!("{}--", self.slug))?;
        (first_label.len() <= MAX_DNS_LABEL && is_label(label))
            .then(|| format!("{label}.{}", self.local_domain))
    }
}

fn is_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// What the Worker says about a request, from the `X-Localify-*` headers.
struct Claims {
    url_id: Option<String>,
    public_host: Option<String>,
    local_host: Option<String>,
    client_ip: Option<IpAddr>,
}

impl Claims {
    /// Reads the Worker's headers, then removes every `X-Localify-*` header
    /// so none of them reaches the app.
    fn take(headers: &mut HeaderMap) -> Self {
        let read = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.trim().to_string())
        };
        let claims = Self {
            url_id: read(URL_ID_HEADER),
            public_host: read(PUBLIC_HOST_HEADER).map(|host| host.to_ascii_lowercase()),
            local_host: read(LOCAL_HOST_HEADER).map(|host| host.to_ascii_lowercase()),
            client_ip: read(CLIENT_IP_HEADER).and_then(|ip| ip.parse().ok()),
        };
        let localify_headers: Vec<HeaderName> = headers
            .keys()
            .filter(|name| name.as_str().starts_with(LOCALIFY_HEADER_PREFIX))
            .cloned()
            .collect();
        for name in localify_headers {
            headers.remove(name);
        }
        claims
    }

    /// Step 4: the local host to route to, when the URL id is served here,
    /// the public host is one of that URL's, the local host is exactly what
    /// it maps to, and the router's registration for it belongs to the same
    /// project as the URL's local domain.
    fn local_host(&self, grant: &IngressGrant, state: &RuntimeState) -> Option<String> {
        let url_id = self.url_id.as_deref()?;
        let url = grant.urls.iter().find(|url| url.id == url_id)?;
        let local_host = url.local_host_for(self.public_host.as_deref()?)?;
        if self.local_host.as_deref() != Some(local_host.as_str()) {
            return None;
        }
        state.public_registration(&local_host, &url.local_domain)?;
        Some(local_host)
    }
}

/// Steps 1 to 5 of the contract: the request to hand to the router, or the
/// status that refuses it.
fn admit(
    state: Arc<RuntimeState>,
    access: &IngressAccess,
    mut request: Request,
) -> Result<Request, StatusCode> {
    let grant = access
        .current()
        .filter(|grant| grant.accepts(request.headers().get(INGRESS_HEADER)));
    let Some(grant) = grant else {
        debug!("Public request refused: no ingress secret yet, or not the right one");
        return Err(StatusCode::FORBIDDEN);
    };
    let claims = Claims::take(request.headers_mut());
    if is_localify_path(request.uri().path()) {
        return Err(StatusCode::NOT_FOUND);
    }
    let Some(local_host) = claims.local_host(&grant, &state) else {
        debug!(
            public_host = claims.public_host.as_deref().unwrap_or(""),
            "Public request refused: not a public URL this machine serves"
        );
        return Err(StatusCode::NOT_FOUND);
    };
    let Ok(host) = HeaderValue::from_str(&local_host) else {
        return Err(StatusCode::NOT_FOUND);
    };
    request.headers_mut().insert(header::HOST, host);
    request.extensions_mut().insert(PublicSource {
        public_host: claims.public_host.unwrap_or_default(),
        client_ip: claims.client_ip,
    });
    request.extensions_mut().insert(AdmittedState(state));
    Ok(request)
}

#[derive(Clone)]
struct Ingress {
    state: SharedState,
    access: IngressAccess,
    permits: Arc<Semaphore>,
}

async fn ingress(State(ingress): State<Ingress>, request: Request, next: Next) -> Response {
    let request = match admit(ingress.state.load_full(), &ingress.access, request) {
        Ok(request) => request,
        Err(status) => return public_error_response(status),
    };
    // Held until the router returns the response head; the body streams
    // after, so SSE and other long responses do not starve the rest.
    let Ok(_permit) = ingress.permits.clone().acquire_owned().await else {
        return public_error_response(StatusCode::SERVICE_UNAVAILABLE);
    };
    next.run(request).await
}

/// The router public traffic goes through: the ingress checks, then the
/// routes local traffic uses.
pub fn ingress_router(state: SharedState, access: IngressAccess) -> Router {
    router_with_permits(state, access, Arc::new(Semaphore::new(MAX_IN_FLIGHT)))
}

fn router_with_permits(
    state: SharedState,
    access: IngressAccess,
    permits: Arc<Semaphore>,
) -> Router {
    let ingress_state = Ingress {
        state: state.clone(),
        access,
        permits,
    };
    create_router(state).layer(middleware::from_fn_with_state(ingress_state, ingress))
}

/// Binds `127.0.0.1:<port>`. The listener stays up whether or not the
/// machine is signed in: without a grant it answers 403 to everything.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await
}

/// Serves public traffic through a full hyper connection, so WebSocket
/// upgrades work, until `cancel` fires.
pub async fn serve(listener: TcpListener, router: Router, cancel: CancellationToken) {
    if let Ok(address) = listener.local_addr() {
        info!(addr = %address, "Public ingress listening");
    }
    loop {
        let stream = tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(error) => {
                    warn!(error = %error, "Public ingress could not accept a connection");
                    continue;
                }
            },
        };
        let _ = stream.set_nodelay(true);
        let service = hyper_util::service::TowerToHyperService::new(router.clone().into_service());
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            if let Err(error) =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(io, service)
                    .await
            {
                debug!(error = %error, "Public ingress connection ended with an error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use arc_swap::ArcSwap;
    use axum::body::Body;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    use super::*;
    use crate::domain::{DomainPattern, DomainRegistration, RegistrationSource, Route};
    use crate::infrastructure::cloud::types::{
        CloudMachine, Protection, ProtectionMode, PublicUrl,
    };

    const SECRET: &str = "3f1c9a7e5b2d4f6081a3c5e7f9b1d3f5a7c9e1b3d5f7091b3d5f7a9c1e3f5a7c";

    fn url(id: &str, slug: &str, local_domain: &str, wildcard: bool) -> PublicUrl {
        PublicUrl {
            id: id.into(),
            slug: slug.into(),
            url: format!("https://{slug}.localify.fyi"),
            local_domain: local_domain.into(),
            wildcard,
            machine: Some(CloudMachine {
                id: "m1".into(),
                name: "laptop".into(),
            }),
            protection: Protection {
                mode: ProtectionMode::Password,
            },
            created_at: "2026-11-05T18:00:00Z".into(),
        }
    }

    fn registration(pattern: &str, port: u16, source: RegistrationSource) -> DomainRegistration {
        let (name, wildcard) = match pattern.strip_prefix("*.") {
            Some(base) => (base, true),
            None => (pattern, false),
        };
        DomainRegistration::with_source(
            DomainPattern::from_name(name, wildcard).unwrap(),
            vec![Route::parse(&format!("/={port}")).unwrap()],
            source,
        )
    }

    /// `*.app.localify` and `blog.localify` belong to project `acme`,
    /// `private.app.localify` and `admin.journal.localify` to project
    /// `other`, `loose.localify` to no project, and `docker.localify` comes
    /// from Docker. Project `journal` is a monorepo: its services register
    /// `web.journal.localify` and `web--feature.journal.localify`, and
    /// nothing registers its namespace `journal.localify`.
    fn state(port: u16) -> SharedState {
        let registrations = vec![
            registration("*.app.localify", port, RegistrationSource::Config),
            registration("private.app.localify", port, RegistrationSource::Config),
            registration("blog.localify", port, RegistrationSource::Config),
            registration("web.journal.localify", port, RegistrationSource::Config),
            registration(
                "web--feature.journal.localify",
                port,
                RegistrationSource::Config,
            ),
            registration("admin.journal.localify", port, RegistrationSource::Config),
            registration("loose.localify", port, RegistrationSource::Config),
            registration("docker.localify", port, RegistrationSource::External),
        ];
        let projects = HashMap::from([
            ("*.app.localify".to_string(), "acme".to_string()),
            ("private.app.localify".to_string(), "other".to_string()),
            ("blog.localify".to_string(), "acme".to_string()),
            ("web.journal.localify".to_string(), "journal".to_string()),
            (
                "web--feature.journal.localify".to_string(),
                "journal".to_string(),
            ),
            ("admin.journal.localify".to_string(), "other".to_string()),
            ("docker.localify".to_string(), "acme".to_string()),
        ]);
        // What `Config::exposable_domains` gives: each project domain by its
        // base domain, then each project namespace. `docker.localify` is
        // there too, to show the router's registration check still refuses
        // Docker's registration.
        let exposable = HashMap::from([
            ("app.localify".to_string(), "acme".to_string()),
            ("private.app.localify".to_string(), "other".to_string()),
            ("blog.localify".to_string(), "acme".to_string()),
            ("web.journal.localify".to_string(), "journal".to_string()),
            (
                "web--feature.journal.localify".to_string(),
                "journal".to_string(),
            ),
            ("admin.journal.localify".to_string(), "other".to_string()),
            ("docker.localify".to_string(), "acme".to_string()),
            ("acme.localify".to_string(), "acme".to_string()),
            ("journal.localify".to_string(), "journal".to_string()),
            ("other.localify".to_string(), "other".to_string()),
        ]);
        Arc::new(ArcSwap::from_pointee(
            RuntimeState::with_project_roots_and_access_log(
                registrations,
                HashMap::new(),
                Default::default(),
            )
            .with_domain_projects(projects)
            .with_exposable_domains(exposable),
        ))
    }

    fn access() -> IngressAccess {
        let access = IngressAccess::default();
        access.grant(IngressGrant::new(
            SECRET,
            &[
                url("u-app", "app-a8f3", "app.localify", true),
                url("u-blog", "blog-1b2c", "blog.localify", false),
                url("u-journal", "journal-a8f3", "journal.localify", true),
                url("u-loose", "loose-9d9d", "loose.localify", false),
                url("u-docker", "docker-0e0e", "docker.localify", false),
            ],
        ));
        access
    }

    fn public_request(
        url_id: &str,
        public_host: &str,
        local_host: &str,
    ) -> axum::http::request::Builder {
        Request::builder()
            .uri("/page?x=1")
            .header(header::HOST, "m-1.localify.dev")
            .header(INGRESS_HEADER, SECRET)
            .header(URL_ID_HEADER, url_id)
            .header(PUBLIC_HOST_HEADER, public_host)
            .header(LOCAL_HOST_HEADER, local_host)
            .header(CLIENT_IP_HEADER, "203.0.113.7")
    }

    fn check(request: axum::http::request::Builder) -> Result<Request, StatusCode> {
        admit(
            state(3000).load_full(),
            &access(),
            request.body(Body::empty()).unwrap(),
        )
    }

    #[test]
    fn a_request_without_a_grant_is_forbidden() {
        let request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify")
            .body(Body::empty())
            .unwrap();

        let refusal =
            admit(state(3000).load_full(), &IngressAccess::default(), request).unwrap_err();

        assert_eq!(refusal, StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_wrong_or_missing_secret_is_forbidden() {
        let mut request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify");
        request
            .headers_mut()
            .unwrap()
            .insert(INGRESS_HEADER, HeaderValue::from_static("not-the-secret"));
        assert_eq!(check(request).unwrap_err(), StatusCode::FORBIDDEN);

        let mut request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify");
        request.headers_mut().unwrap().remove(INGRESS_HEADER);
        assert_eq!(check(request).unwrap_err(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn an_admitted_request_carries_the_local_host_and_no_localify_header() {
        let request = public_request("u-blog", "Blog-1b2c.localify.fyi", "blog.localify")
            .header("x-localify-local-domain", "spoofed.localify")
            .header("x-localify-anything", "1");

        let request = check(request).unwrap();

        assert_eq!(request.headers()[header::HOST], "blog.localify");
        assert!(
            request
                .headers()
                .keys()
                .all(|name| !name.as_str().starts_with(LOCALIFY_HEADER_PREFIX))
        );
        let source = request.extensions().get::<PublicSource>().unwrap();
        assert_eq!(source.public_host, "blog-1b2c.localify.fyi");
        assert_eq!(source.client_ip, Some("203.0.113.7".parse().unwrap()));
    }

    #[test]
    fn localify_paths_are_not_found() {
        for path in [
            "/.localify/open-in-editor",
            "/.localify/run-dev-server",
            "/.localify/launch-profile.js",
            "/.localify",
        ] {
            let request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify")
                .uri(path)
                .method("POST");
            assert_eq!(check(request).unwrap_err(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[test]
    fn a_wildcard_url_serves_its_labels() {
        let request = public_request(
            "u-app",
            "app-a8f3--feature--x.localify.fyi",
            "feature--x.app.localify",
        );

        let request = check(request).unwrap();

        assert_eq!(request.headers()[header::HOST], "feature--x.app.localify");
    }

    #[test]
    fn a_wildcard_url_never_reaches_another_projects_more_specific_domain() {
        // `private.app.localify` matches the wildcard's label syntax, but the
        // router selects project `other`'s exact registration for it.
        let request = public_request(
            "u-app",
            "app-a8f3--private.localify.fyi",
            "private.app.localify",
        );

        assert_eq!(check(request).unwrap_err(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_wildcard_url_on_a_namespace_reaches_the_projects_services() {
        // Nothing registers `journal.localify`; the namespace makes `journal`
        // the URL's project.
        let request = public_request(
            "u-journal",
            "journal-a8f3--web--feature.localify.fyi",
            "web--feature.journal.localify",
        );

        let request = check(request).unwrap();

        assert_eq!(
            request.headers()[header::HOST],
            "web--feature.journal.localify"
        );
    }

    #[test]
    fn a_wildcard_url_on_a_namespace_never_reaches_another_project() {
        let request = public_request(
            "u-journal",
            "journal-a8f3--admin.localify.fyi",
            "admin.journal.localify",
        );

        assert_eq!(check(request).unwrap_err(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn requests_outside_the_inventory_are_not_found() {
        let cases = [
            // A URL id this machine does not serve.
            ("u-unknown", "blog-1b2c.localify.fyi", "blog.localify"),
            // Another URL's public host.
            ("u-blog", "app-a8f3.localify.fyi", "blog.localify"),
            // A label on a URL that is not a wildcard.
            ("u-blog", "blog-1b2c--x.localify.fyi", "x.blog.localify"),
            // A local host the public host does not map to.
            ("u-app", "app-a8f3--feature.localify.fyi", "blog.localify"),
            (
                "u-app",
                "app-a8f3--feature.localify.fyi",
                "other.app.localify",
            ),
            // Invalid labels.
            ("u-app", "app-a8f3---x.localify.fyi", "-x.app.localify"),
            ("u-app", "app-a8f3--a.b.localify.fyi", "a.b.app.localify"),
            // The namespace itself, and a label no service registers.
            ("u-journal", "journal-a8f3.localify.fyi", "journal.localify"),
            (
                "u-journal",
                "journal-a8f3--api.localify.fyi",
                "api.journal.localify",
            ),
            // A domain of no project, and a Docker domain.
            ("u-loose", "loose-9d9d.localify.fyi", "loose.localify"),
            ("u-docker", "docker-0e0e.localify.fyi", "docker.localify"),
        ];
        for (url_id, public_host, local_host) in cases {
            let request = public_request(url_id, public_host, local_host);
            assert_eq!(
                check(request).unwrap_err(),
                StatusCode::NOT_FOUND,
                "{url_id} {public_host} {local_host}"
            );
        }

        let mut request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify");
        request.headers_mut().unwrap().remove(LOCAL_HOST_HEADER);
        assert_eq!(check(request).unwrap_err(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_label_must_fit_one_dns_label() {
        let served = ServedUrl::from_api(&url("u", "app-a8f3", "app.localify", true)).unwrap();
        let fits = "a".repeat(MAX_DNS_LABEL - "app-a8f3--".len());

        assert!(
            served
                .local_host_for(&format!("app-a8f3--{fits}.localify.fyi"))
                .is_some()
        );
        assert!(
            served
                .local_host_for(&format!("app-a8f3--{fits}a.localify.fyi"))
                .is_none()
        );
    }

    /// An upstream that sends its response head when told to, then keeps
    /// the body open until told again.
    async fn slow_upstream() -> (
        u16,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (head_tx, head_rx) = tokio::sync::oneshot::channel::<()>();
        let (end_tx, end_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 4096];
            let _ = stream.read(&mut buffer).await.unwrap();
            let _ = head_rx.await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .await
                .unwrap();
            let _ = end_rx.await;
            let _ = stream.write_all(b"0\r\n\r\n").await;
        });
        (port, head_tx, end_tx)
    }

    #[tokio::test]
    async fn a_permit_is_held_until_the_response_head_arrives() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (port, send_head, end_body) = slow_upstream().await;
        let permits = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let router = router_with_permits(state(port), access(), permits.clone());
        let request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify")
            .body(Body::empty())
            .unwrap();

        let pending = tokio::spawn(router.oneshot(request));
        let waited = wait_for(|| permits.available_permits() == MAX_IN_FLIGHT - 1).await;
        assert!(waited, "the request should hold a permit while it waits");

        send_head.send(()).unwrap();
        let response = pending.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // The body is still open, and the permit is back.
        assert_eq!(permits.available_permits(), MAX_IN_FLIGHT);

        end_body.send(()).unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn a_refused_request_takes_no_permit() {
        let permits = Arc::new(Semaphore::new(1));
        let router = router_with_permits(state(3000), IngressAccess::default(), permits.clone());
        let request = public_request("u-blog", "blog-1b2c.localify.fyi", "blog.localify")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(permits.available_permits(), 1);
    }

    async fn wait_for(condition: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if condition() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }
}
