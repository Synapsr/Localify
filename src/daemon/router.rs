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
use crate::infrastructure::local_dev::open_local_dev_server;

use super::embedded_assets;
use super::proxy::{ClientAddr, Scheme, proxy_request};
use super::static_files::serve_static;
use super::theme;

const OPEN_IN_EDITOR_PATH: &str = "/.localify/open-in-editor";
const RUN_DEV_SERVER_PATH: &str = "/.localify/run-dev-server";
const DOMAIN_NOT_REGISTERED_TEMPLATE: &str = include_str!("templates/domain_not_registered.html");
const NO_ROUTE_FOUND_TEMPLATE: &str = include_str!("templates/no_route_found.html");
const ACTION_ERROR_TEMPLATE: &str = include_str!("templates/action_error.html");

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

    if method == Method::POST && path == RUN_DEV_SERVER_PATH {
        return handle_run_dev_server(
            &state,
            host_header.as_deref(),
            proto,
            client_ip,
            uri.query(),
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

fn handle_run_dev_server(
    state: &RuntimeState,
    host_header: Option<&str>,
    scheme: &str,
    client_ip: Option<IpAddr>,
    target_query: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    let host = match host_header {
        Some(host) => host,
        None => return action_error_response(StatusCode::BAD_REQUEST, "Missing Host header."),
    };

    if !client_ip.is_some_and(|ip| ip.is_loopback()) {
        return action_error_response(
            StatusCode::FORBIDDEN,
            "Run dev server is only available from this machine.",
        );
    }

    if !is_same_origin_action(headers, scheme, host) {
        return action_error_response(
            StatusCode::FORBIDDEN,
            "Run dev server requires a same-origin request from the Localify error page.",
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
    let Some(target) = query_param_value(target_query, "target").or_else(|| {
        registration
            .routes()
            .iter()
            .find_map(|route| match route.target() {
                RouteTarget::Proxy(target) => Some(target.to_string()),
                RouteTarget::StaticFiles(_) => None,
            })
    }) else {
        return action_error_response(
            StatusCode::BAD_REQUEST,
            "This domain is not backed by a proxy target that Localify can launch.",
        );
    };

    match open_local_dev_server(Path::new(root_path), &target) {
        Ok(()) => redirect_back(headers, scheme, host),
        Err(error) => action_error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

fn build_not_registered_response(domain: &str) -> Response {
    let domain = domain.split(':').next().unwrap_or(domain);
    let domain_raw = domain.trim_end_matches('.').to_lowercase();
    let domain = theme::html_escape(&domain_raw);
    let image_data_uri = embedded_assets::roxy_error_data_uri();
    let app_actions = build_open_app_actions("localify://open/projects");
    let register_commands = build_domain_registration_commands(&domain_raw, &domain);
    let restart_command = format!("{} restart", BRANDING.cli_command());
    let body = theme::render_template(
        DOMAIN_NOT_REGISTERED_TEMPLATE,
        &[
            ("{{IMAGE_DATA_URI}}", image_data_uri),
            ("{{DOMAIN}}", &domain),
            ("{{APP_NAME}}", BRANDING.app_name),
            ("{{APP_ACTIONS}}", &app_actions),
            ("{{REGISTER_COMMANDS}}", &register_commands),
            ("{{RESTART_COMMAND}}", &restart_command),
        ],
    );

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
    let body = theme::render_template(ACTION_ERROR_TEMPLATE, &[("{{MESSAGE}}", &message)]);
    html_response(
        status,
        theme::render_page(
            "Localify Action Unavailable",
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

fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    query.and_then(|query| {
        query.split('&').find_map(|pair| {
            let (candidate, value) = pair.split_once('=')?;
            (candidate == key).then(|| percent_decode(value)).flatten()
        })
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                let decoded = u8::from_str_radix(hex, 16).ok()?;
                out.push(decoded as char);
                index += 3;
            }
            b'+' => {
                out.push(' ');
                index += 1;
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }

    Some(out)
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
    let domain_raw = domain.trim_end_matches('.').to_lowercase();
    let domain = theme::html_escape(&domain_raw);
    let path = theme::html_escape(path);
    let reg_domain = theme::html_escape(registration.domain().as_str());
    let image_data_uri = embedded_assets::roxy_404_data_uri();
    let app_actions = build_open_app_actions(&format!(
        "localify://open/logs?domain={}",
        theme::encode_path_segment(&domain_raw)
    ));
    let add_route_command = build_add_route_command(registration.is_wildcard(), &reg_domain, &path);
    let reload_command = format!("{} reload", BRANDING.cli_command());
    let body = theme::render_template(
        NO_ROUTE_FOUND_TEMPLATE,
        &[
            ("{{IMAGE_DATA_URI}}", image_data_uri),
            ("{{PATH}}", &path),
            ("{{DOMAIN}}", &domain),
            ("{{APP_ACTIONS}}", &app_actions),
            ("{{ADD_ROUTE_COMMAND}}", &add_route_command),
            ("{{APP_NAME}}", BRANDING.app_name),
            ("{{RELOAD_COMMAND}}", &reload_command),
        ],
    );

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

fn build_open_app_actions(href: &str) -> String {
    format!(
        "<div class=\"error-actions\">\
<a class=\"error-button\" href=\"{href}\">Open in app</a>\
</div>"
    )
}

fn wildcard_base_domain(domain: &str) -> Option<String> {
    let domain = domain.trim_end_matches('.');
    if !domain.ends_with(crate::branding::BRANDING.domain_suffix()) {
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

fn build_domain_registration_commands(domain_raw: &str, escaped_domain: &str) -> String {
    let mut commands = String::new();
    commands.push_str(BRANDING.cli_command());
    commands.push_str(" register ");
    commands.push_str(escaped_domain);
    commands.push_str(" --route \"/=3000\"<br>");

    if let Some(wildcard_base) = wildcard_base_domain(domain_raw) {
        let wildcard_base = theme::html_escape(&wildcard_base);
        commands
            .push_str("<span class=\"comment\"># using subdomains? register wildcard:</span><br>");
        commands.push_str(BRANDING.cli_command());
        commands.push_str(" register ");
        commands.push_str(&wildcard_base);
        commands.push_str(" --wildcard --route \"/=3000\"<br>");
    }

    commands.push_str("<span class=\"comment\"># or with multiple routes:</span><br>");
    commands.push_str(BRANDING.cli_command());
    commands.push_str(" register ");
    commands.push_str(escaped_domain);
    commands.push_str(" --route \"/=3000\" --route \"/api=3001\"");
    commands
}

fn build_add_route_command(is_wildcard: bool, reg_domain: &str, path: &str) -> String {
    let mut command = String::new();
    command.push_str(BRANDING.cli_command());
    command.push_str(" route add ");
    if is_wildcard {
        command.push_str("--wildcard ");
    }
    command.push_str(reg_domain);
    command.push(' ');
    command.push_str(path);
    command.push_str(" 3000");
    command
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{RouteResolution, RuntimeState, resolve_route};
    use crate::branding::BRANDING;
    use crate::domain::{DomainName, DomainPattern, DomainRegistration, Route};

    fn reg(domain: &str, wildcard: bool) -> DomainRegistration {
        let normalized = domain.replace(".roxy", BRANDING.domain_suffix());
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
