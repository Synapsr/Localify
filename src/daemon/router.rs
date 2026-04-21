use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use axum::{
    Extension, Router,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use tracing::{debug, info};

use crate::branding::BRANDING;
use crate::domain::{DomainRegistration, Route, RouteTarget};
use crate::infrastructure::editor::open_in_editor;

use super::embedded_assets;
use super::proxy::{ClientAddr, Scheme, proxy_request};
use super::static_files::serve_static;
use super::theme;

const OPEN_IN_EDITOR_PATH: &str = "/.localify/open-in-editor";

/// Immutable snapshot of daemon state. Swapped atomically via `SharedState`.
pub struct RuntimeState {
    /// All registrations sorted by pattern specificity (most specific first).
    registrations: Vec<DomainRegistration>,
    project_roots: HashMap<String, String>,
}

impl RuntimeState {
    #[cfg(test)]
    pub fn new(registrations: Vec<DomainRegistration>) -> Self {
        Self::with_project_roots(registrations, HashMap::new())
    }

    pub fn with_project_roots(
        mut registrations: Vec<DomainRegistration>,
        project_roots: HashMap<String, String>,
    ) -> Self {
        // Most-specific first: longer base domain wins.
        // At equal specificity, exact patterns come before wildcards.
        registrations.sort_by(|a, b| {
            b.pattern()
                .specificity()
                .cmp(&a.pattern().specificity())
                .then_with(|| a.is_wildcard().cmp(&b.is_wildcard()))
        });

        Self {
            registrations,
            project_roots,
        }
    }

    pub fn get_domain(&self, host: &str) -> Option<&DomainRegistration> {
        // Strip port from host if present
        let domain = host.split(':').next().unwrap_or(host);
        let domain = domain.trim_end_matches('.').to_lowercase();

        self.registrations
            .iter()
            .find(|r| r.pattern().matches_hostname(&domain))
    }

    pub fn registrations(&self) -> &[DomainRegistration] {
        &self.registrations
    }

    pub fn project_root_for_registration(&self, registration: &DomainRegistration) -> Option<&str> {
        self.project_roots
            .get(&registration.display_pattern())
            .map(String::as_str)
    }
}

/// Shared, atomically-swappable runtime state. Lock-free reads.
pub type SharedState = Arc<ArcSwap<RuntimeState>>;

/// Pure result of resolving a request to a route. No I/O.
#[derive(Debug)]
pub enum RouteResolution<'a> {
    Matched {
        registration: &'a DomainRegistration,
        route: &'a Route,
    },
    MissingHost,
    DomainNotRegistered {
        host: String,
    },
    NoRouteForPath {
        registration: &'a DomainRegistration,
        host: String,
        path: String,
    },
}

/// Pure route resolution — no I/O, no async.
pub fn resolve_route<'a>(
    state: &'a RuntimeState,
    host_header: Option<&str>,
    path: &str,
) -> RouteResolution<'a> {
    let host = match host_header {
        Some(h) => h.to_string(),
        None => return RouteResolution::MissingHost,
    };
    let registration = match state.get_domain(&host) {
        Some(r) => r,
        None => return RouteResolution::DomainNotRegistered { host },
    };
    match registration.match_route(path) {
        Some(route) => RouteResolution::Matched {
            registration,
            route,
        },
        None => RouteResolution::NoRouteForPath {
            registration,
            host,
            path: path.to_string(),
        },
    }
}

/// Extract host from request headers
fn get_host(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

/// Create the main router
pub fn create_router(state: SharedState) -> Router {
    Router::new()
        .route("/{*path}", any(handle_request))
        .route("/", any(handle_request))
        .with_state(state)
}

/// Handle all incoming requests
async fn handle_request(
    State(shared): State<SharedState>,
    scheme: Option<Extension<Scheme>>,
    client_addr: Option<Extension<ClientAddr>>,
    request: Request,
) -> Response {
    let state = shared.load();
    let method = request.method().clone();
    let uri = request.uri().clone();
    let host_header = get_host(&request);
    let path = uri.path();
    let proto = scheme.map(|Extension(s)| s.as_str()).unwrap_or("http");
    let client_ip = client_addr.map(|Extension(a)| a.0);

    if method == Method::POST && path == OPEN_IN_EDITOR_PATH {
        return handle_open_in_editor(
            &state,
            host_header.as_deref(),
            proto,
            client_ip,
            request.headers(),
        );
    }

    match resolve_route(&state, host_header.as_deref(), path) {
        RouteResolution::MissingHost => {
            (StatusCode::BAD_REQUEST, "Missing Host header").into_response()
        }
        RouteResolution::DomainNotRegistered { host } => {
            info!(host = %host, "Domain not registered");
            build_not_registered_response(&host)
        }
        RouteResolution::NoRouteForPath {
            registration,
            host,
            path,
        } => {
            info!(host = %host, path = %path, "No route found");
            build_no_route_response(registration, &host, &path)
        }
        RouteResolution::Matched {
            registration,
            route,
        } => {
            let host = host_header.as_deref().unwrap_or("");
            debug!(
                method = %method,
                host = %host,
                domain = %registration.domain(),
                path = %path,
                route = %route.path(),
                "Routing request"
            );

            let response = match route.target() {
                RouteTarget::StaticFiles(dir) => {
                    serve_static(route.path().as_str(), dir.clone(), request).await
                }
                RouteTarget::Proxy(target) => {
                    proxy_request(
                        target,
                        request,
                        host,
                        proto,
                        client_ip,
                        state.project_root_for_registration(registration),
                    )
                    .await
                }
            };

            info!(
                method = %method,
                host = %host,
                path = %path,
                status = response.status().as_u16(),
                "Request completed"
            );

            response
        }
    }
}

fn handle_open_in_editor(
    state: &RuntimeState,
    host_header: Option<&str>,
    scheme: &str,
    client_ip: Option<IpAddr>,
    headers: &HeaderMap,
) -> Response {
    let host = match host_header {
        Some(host) => host,
        None => return action_error_response(StatusCode::BAD_REQUEST, "Missing Host header."),
    };

    if !client_ip.is_some_and(|ip| ip.is_loopback()) {
        return action_error_response(
            StatusCode::FORBIDDEN,
            "Open in editor is only available from this machine.",
        );
    }

    if !is_same_origin_action(headers, scheme, host) {
        return action_error_response(
            StatusCode::FORBIDDEN,
            "Open in editor requires a same-origin request from the Localify error page.",
        );
    }

    let registration = match state.get_domain(host) {
        Some(registration) => registration,
        None => {
            return action_error_response(
                StatusCode::NOT_FOUND,
                "This Localify domain is not registered.",
            );
        }
    };
    let Some(root_path) = state.project_root_for_registration(registration) else {
        return action_error_response(
            StatusCode::NOT_FOUND,
            "No project folder is linked to this domain yet.",
        );
    };

    match open_in_editor(Path::new(root_path)) {
        Ok(()) => redirect_back(headers, scheme, host),
        Err(error) => action_error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

fn build_not_registered_response(domain: &str) -> Response {
    let domain = domain.split(':').next().unwrap_or(domain);
    let domain_raw = domain.trim_end_matches('.').to_lowercase();
    let domain = theme::html_escape(&domain_raw);
    let image_data_uri = embedded_assets::roxy_error_data_uri();

    let mut body = String::new();
    body.push_str("<div class=\"error-container\">\n");
    body.push_str("<div class=\"error-image\">\n");
    body.push_str("<img src=\"");
    body.push_str(image_data_uri);
    body.push_str("\" alt=\"Server Error - Localify Fox\" ");
    body.push_str("width=\"300\" height=\"225\">\n");
    body.push_str("</div>\n");
    body.push_str("<div class=\"error-card\">\n");
    body.push_str("<h1 class=\"error-title\">Domain Not Registered</h1>\n");
    body.push_str("<p class=\"error-message\">The domain <code>");
    body.push_str(&domain);
    body.push_str("</code> is not registered with ");
    body.push_str(BRANDING.app_name);
    body.push_str(".</p>\n");
    body.push_str("<div class=\"help-section\">\n");
    body.push_str("<p class=\"help-label\">To register this domain, run:</p>\n");
    body.push_str("<div class=\"command\">");
    body.push_str(BRANDING.cli_command());
    body.push_str(" register ");
    body.push_str(&domain);
    body.push_str(" --route \"/=3000\"<br>");
    // Suggest wildcard registration when the user hits a subdomain.
    if let Some(wildcard_base) = wildcard_base_domain(&domain_raw) {
        let wildcard_base = theme::html_escape(&wildcard_base);
        body.push_str("<span class=\"comment\"># using subdomains? register wildcard:</span><br>");
        body.push_str(BRANDING.cli_command());
        body.push_str(" register ");
        body.push_str(&wildcard_base);
        body.push_str(" --wildcard --route \"/=3000\"<br>");
    }
    body.push_str("<span class=\"comment\"># or with multiple routes:</span><br>");
    body.push_str(BRANDING.cli_command());
    body.push_str(" register ");
    body.push_str(&domain);
    body.push_str(" --route \"/=3000\" --route \"/api=3001\"");
    body.push_str("</div>\n");
    body.push_str("<p class=\"help-label\">Then restart the ");
    body.push_str(BRANDING.app_name);
    body.push_str(" daemon:</p>\n");
    body.push_str("<div class=\"command\">");
    body.push_str(BRANDING.cli_command());
    body.push_str(" restart</div>\n");
    body.push_str("</div></div></div>");

    let html = theme::render_page("Domain Not Registered", &body, theme::ERROR_PAGE_CSS, "");

    html_response(StatusCode::NOT_FOUND, html)
}

fn is_same_origin_action(headers: &HeaderMap, scheme: &str, host: &str) -> bool {
    let expected_origin = format!("{scheme}://{host}");

    if let Some(origin) = header_value(headers, header::ORIGIN) {
        return origin == expected_origin;
    }

    header_value(headers, header::REFERER)
        .map(|referer| {
            referer == expected_origin || referer.starts_with(&format!("{expected_origin}/"))
        })
        .unwrap_or(false)
}

fn redirect_back(headers: &HeaderMap, scheme: &str, host: &str) -> Response {
    let location = header_value(headers, header::REFERER)
        .and_then(|referer| same_origin_path(&referer, scheme, host))
        .unwrap_or_else(|| "/".to_string());

    Response::builder()
        .status(StatusCode::SEE_OTHER)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| {
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, header::HeaderValue::from_static("/"))],
            )
                .into_response()
        })
}

fn action_error_response(status: StatusCode, message: &str) -> Response {
    let message = theme::html_escape(message);
    let body = format!(
        "<div class=\"error-container\">\
            <div class=\"error-card\">\
                <h1 class=\"error-title\">Open in Editor Unavailable</h1>\
                <p class=\"error-message\">{message}</p>\
            </div>\
        </div>"
    );
    html_response(
        status,
        theme::render_page(
            "Open in Editor Unavailable",
            &body,
            theme::ERROR_PAGE_CSS,
            "",
        ),
    )
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn same_origin_path(url: &str, scheme: &str, host: &str) -> Option<String> {
    let expected_origin = format!("{scheme}://{host}");
    let remainder = url.strip_prefix(&expected_origin)?;
    if remainder.is_empty() {
        return Some("/".to_string());
    }
    if remainder.starts_with('/') {
        return Some(remainder.to_string());
    }
    None
}

fn build_no_route_response(registration: &DomainRegistration, host: &str, path: &str) -> Response {
    let domain = host.split(':').next().unwrap_or(host);
    let domain = domain.trim_end_matches('.').to_lowercase();
    let domain = theme::html_escape(&domain);
    let path = theme::html_escape(path);
    let reg_domain = theme::html_escape(registration.domain().as_str());
    let image_data_uri = embedded_assets::roxy_404_data_uri();

    let mut body = String::new();
    body.push_str("<div class=\"error-container\">\n");
    body.push_str("<div class=\"error-image\">\n");
    body.push_str("<img src=\"");
    body.push_str(image_data_uri);
    body.push_str("\" alt=\"404 - Localify Fox\" ");
    body.push_str("width=\"300\" height=\"225\">\n");
    body.push_str("</div>\n");
    body.push_str("<div class=\"error-card\">\n");
    body.push_str("<h1 class=\"error-title\">No Route Found</h1>\n");
    body.push_str("<p class=\"error-message\">No route matches path <code>");
    body.push_str(&path);
    body.push_str("</code> on domain <code>");
    body.push_str(&domain);
    body.push_str("</code>.</p>\n");
    body.push_str("<div class=\"help-section\">\n");
    body.push_str("<p class=\"help-label\">To add a route for this path, run:</p>\n");
    body.push_str("<div class=\"command\">");
    body.push_str(BRANDING.cli_command());
    body.push_str(" route add ");
    if registration.is_wildcard() {
        body.push_str("--wildcard ");
    }
    body.push_str(&reg_domain);
    body.push(' ');
    body.push_str(&path);
    body.push_str(" 3000</div>\n");
    body.push_str("<p class=\"help-label\">Then reload the ");
    body.push_str(BRANDING.app_name);
    body.push_str(" daemon:</p>\n");
    body.push_str("<div class=\"command\">");
    body.push_str(BRANDING.cli_command());
    body.push_str(" reload</div>\n");
    body.push_str("</div></div></div>");

    let html = theme::render_page("No Route Found", &body, theme::ERROR_PAGE_CSS, "");

    html_response(StatusCode::NOT_FOUND, html)
}

/// Build an HTML response, falling back to a plain-text error if the builder fails.
fn html_response(status: StatusCode, html: String) -> Response {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(axum::body::Body::from(html))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
        })
}

fn wildcard_base_domain(domain: &str) -> Option<String> {
    let domain = domain.trim_end_matches('.');
    if !domain.ends_with(crate::branding::BRANDING.domain_suffix) {
        return None;
    }

    let parts: Vec<&str> = domain.split('.').collect();
    if parts.len() < 3 {
        // myapp.roxy has len=2; no subdomain.
        return None;
    }

    Some(format!(
        "{}.{}",
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{RouteResolution, RuntimeState, resolve_route};
    use crate::branding::BRANDING;
    use crate::domain::{DomainName, DomainPattern, DomainRegistration, Route};

    fn reg(domain: &str, wildcard: bool) -> DomainRegistration {
        let normalized = domain.replace(".roxy", BRANDING.domain_suffix);
        let domain = DomainName::new(normalized).unwrap();
        let pattern = if wildcard {
            DomainPattern::Wildcard(domain)
        } else {
            DomainPattern::Exact(domain)
        };
        let routes = vec![Route::parse("/=3000").unwrap()];
        DomainRegistration::new(pattern, routes)
    }

    #[test]
    fn test_exact_overrides_wildcard_for_base_domain() {
        let wildcard = reg("myapp.roxy", true);
        let exact = reg("myapp.roxy", false);
        let state = RuntimeState::new(vec![wildcard, exact]);

        let found = state.get_domain("myapp.localify").unwrap();
        // Exact patterns match before wildcards because both match,
        // but the exact match is returned since it's found first.
        assert!(!found.is_wildcard());
    }

    #[test]
    fn test_wildcard_matches_subdomain() {
        let wildcard = reg("myapp.roxy", true);
        let state = RuntimeState::new(vec![wildcard]);

        let found = state.get_domain("blog.myapp.localify").unwrap();
        assert!(found.is_wildcard());
        assert_eq!(found.domain().as_str(), "myapp.localify");
    }

    #[test]
    fn runtime_state_returns_project_root_for_registration() {
        let registration = reg("app.roxy", false);
        let mut project_roots = HashMap::new();
        project_roots.insert("app.localify".to_string(), "/tmp/app".to_string());

        let state = RuntimeState::with_project_roots(vec![registration], project_roots);
        let matched = state.get_domain("app.localify").unwrap();

        assert_eq!(
            state.project_root_for_registration(matched),
            Some("/tmp/app")
        );
    }

    #[test]
    fn same_origin_action_accepts_matching_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.localify"),
        );

        assert!(super::is_same_origin_action(
            &headers,
            "https",
            "app.localify"
        ));
    }

    #[test]
    fn same_origin_action_rejects_cross_site_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );

        assert!(!super::is_same_origin_action(
            &headers,
            "https",
            "app.localify"
        ));
    }

    #[test]
    fn test_wildcard_does_not_match_multi_level_subdomain() {
        let wildcard = reg("myapp.roxy", true);
        let state = RuntimeState::new(vec![wildcard]);

        // This previously matched (bug DA3) — now fixed by DomainPattern
        assert!(state.get_domain("a.b.myapp.localify").is_none());
    }

    #[test]
    fn test_most_specific_wildcard_wins() {
        let broad = reg("myapp.roxy", true);
        let specific = reg("sub.myapp.roxy", true);
        let state = RuntimeState::new(vec![broad, specific]);

        let found = state.get_domain("blog.sub.myapp.localify").unwrap();
        assert_eq!(found.domain().as_str(), "sub.myapp.localify");
    }

    #[test]
    fn test_host_is_normalized_for_lookup() {
        let exact = reg("app.roxy", false);
        let state = RuntimeState::new(vec![exact]);

        assert!(state.get_domain("APP.LOCALIFY:443").is_some());
    }

    // --- resolve_route tests ---

    #[test]
    fn resolve_route_missing_host() {
        let state = RuntimeState::new(vec![reg("app.roxy", false)]);
        assert!(matches!(
            resolve_route(&state, None, "/"),
            RouteResolution::MissingHost
        ));
    }

    #[test]
    fn resolve_route_unknown_domain() {
        let state = RuntimeState::new(vec![reg("app.roxy", false)]);
        match resolve_route(&state, Some("other.localify"), "/") {
            RouteResolution::DomainNotRegistered { host } => {
                assert_eq!(host, "other.localify");
            }
            other => panic!("expected DomainNotRegistered, got {:?}", other),
        }
    }

    #[test]
    fn resolve_route_matched() {
        let state = RuntimeState::new(vec![reg("app.roxy", false)]);
        match resolve_route(&state, Some("app.localify"), "/") {
            RouteResolution::Matched {
                registration,
                route,
            } => {
                assert_eq!(registration.domain().as_str(), "app.localify");
                assert_eq!(route.path().as_str(), "/");
            }
            other => panic!("expected Matched, got {:?}", other),
        }
    }

    #[test]
    fn resolve_route_no_matching_route() {
        let domain = DomainName::new("app.localify").unwrap();
        let pattern = DomainPattern::Exact(domain);
        let routes = vec![Route::parse("/api=3000").unwrap()];
        let registration = DomainRegistration::new(pattern, routes);

        let state = RuntimeState::new(vec![registration]);
        match resolve_route(&state, Some("app.localify"), "/other") {
            RouteResolution::NoRouteForPath {
                registration: reg,
                host,
                path,
            } => {
                assert_eq!(reg.domain().as_str(), "app.localify");
                assert_eq!(host, "app.localify");
                assert_eq!(path, "/other");
            }
            other => panic!("expected NoRouteForPath, got {:?}", other),
        }
    }

    #[test]
    fn resolve_route_strips_port() {
        let state = RuntimeState::new(vec![reg("app.roxy", false)]);
        assert!(matches!(
            resolve_route(&state, Some("app.localify:8080"), "/"),
            RouteResolution::Matched { .. }
        ));
    }

    // --- RuntimeState tests ---

    #[test]
    fn runtime_state_registrations_accessor() {
        let state = RuntimeState::new(vec![reg("b.roxy", false), reg("a.roxy", false)]);
        let regs = state.registrations();
        assert_eq!(regs.len(), 2);
        // Both present — sorted by specificity (equal length, so order is stable)
        let domains: Vec<&str> = regs.iter().map(|r| r.domain().as_str()).collect();
        assert!(domains.contains(&"a.localify"));
        assert!(domains.contains(&"b.localify"));
    }

    #[test]
    fn runtime_state_swap_replaces_registrations() {
        use super::SharedState;
        use arc_swap::ArcSwap;
        use std::sync::Arc;

        let original = RuntimeState::new(vec![reg("old.roxy", false)]);
        let shared: SharedState = Arc::new(ArcSwap::from_pointee(original));

        // Verify original state
        assert!(shared.load().get_domain("old.localify").is_some());
        assert!(shared.load().get_domain("new.localify").is_none());

        // Swap to new state
        let replacement = Arc::new(RuntimeState::new(vec![reg("new.roxy", false)]));
        shared.store(replacement);

        // Verify new state
        assert!(shared.load().get_domain("old.localify").is_none());
        assert!(shared.load().get_domain("new.localify").is_some());
    }

    #[tokio::test]
    async fn runtime_state_reload_via_channel() {
        use super::SharedState;
        use arc_swap::ArcSwap;
        use std::sync::Arc;
        use tokio::sync::mpsc;

        let original = RuntimeState::new(vec![reg("old.roxy", false)]);
        let shared: SharedState = Arc::new(ArcSwap::from_pointee(original));

        let (tx, mut rx) = mpsc::channel::<Vec<crate::domain::DomainRegistration>>(4);

        // Simulate what the server's reload listener does
        let state_for_reload = shared.clone();
        let handle = tokio::spawn(async move {
            if let Some(regs) = rx.recv().await {
                let new = Arc::new(RuntimeState::new(regs));
                state_for_reload.store(new);
            }
        });

        // Send new registrations through the channel
        tx.send(vec![reg("new.roxy", false)]).await.unwrap();
        handle.await.unwrap();

        assert!(shared.load().get_domain("old.localify").is_none());
        assert!(shared.load().get_domain("new.localify").is_some());
    }
}
