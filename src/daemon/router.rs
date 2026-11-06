use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use axum::{
    Extension, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use tracing::{debug, field, info};

use crate::branding::BRANDING;
use crate::domain::{DomainRegistration, ProxyTarget, RegistrationSource, Route, RouteTarget};
use crate::infrastructure::editor::open_in_editor;
use crate::infrastructure::launch_sessions::LAUNCH_PROFILE_HEADER;

use super::embedded_assets;
use super::launch_sessions::LaunchSessions;
use super::profile_indicator;
use super::proxy::{ClientAddr, PublicSource, Scheme, proxy_request, public_error_response};
use super::static_files::serve_static;
use super::theme;

/// Localify's own endpoints. Public traffic never reaches them.
const LOCALIFY_PATH_PREFIX: &str = "/.localify/";
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
    /// Project id of each config domain that belongs to a project, by
    /// pattern. Only these domains can be public.
    domain_projects: HashMap<String, String>,
    /// Project id of each exposable local domain, from
    /// `Config::exposable_domains`.
    exposable_domains: HashMap<String, String>,
    domain_accesses: DomainAccessLog,
    launch_sessions: LaunchSessions,
}

impl RuntimeState {
    #[cfg(test)]
    pub fn new(registrations: Vec<DomainRegistration>) -> Self {
        Self::with_project_roots(registrations, HashMap::new())
    }

    #[cfg(test)]
    pub fn with_project_roots(
        registrations: Vec<DomainRegistration>,
        project_roots: HashMap<String, String>,
    ) -> Self {
        Self::with_project_roots_and_access_log(
            registrations,
            project_roots,
            DomainAccessLog::default(),
        )
    }

    pub fn with_project_roots_and_access_log(
        mut registrations: Vec<DomainRegistration>,
        project_roots: HashMap<String, String>,
        domain_accesses: DomainAccessLog,
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
            domain_projects: HashMap::new(),
            exposable_domains: HashMap::new(),
            domain_accesses,
            launch_sessions: LaunchSessions::default(),
        }
    }

    pub fn with_domain_projects(mut self, domain_projects: HashMap<String, String>) -> Self {
        self.domain_projects = domain_projects;
        self
    }

    pub fn with_exposable_domains(mut self, exposable_domains: HashMap<String, String>) -> Self {
        self.exposable_domains = exposable_domains;
        self
    }

    /// Keeps the launch session registry across config reloads.
    pub fn with_launch_sessions(mut self, launch_sessions: LaunchSessions) -> Self {
        self.launch_sessions = launch_sessions;
        self
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

    /// The registration the router selects for `local_host`, when public
    /// traffic for a URL on `url_local_domain` may reach it: a config domain
    /// of the URL's project. Without the project check, a wildcard URL on
    /// `app.localify` would reach another project's more specific
    /// `private.app.localify`. Standalone, Docker and other external domains
    /// are never public.
    pub fn public_registration(
        &self,
        local_host: &str,
        url_local_domain: &str,
    ) -> Option<&DomainRegistration> {
        let selected = self.get_domain(local_host)?;
        let project = self.project_of(selected)?;
        (self.url_project(url_local_domain) == Some(project)).then_some(selected)
    }

    /// The project of a public URL on `local_domain`
    /// (`docs/public-urls.md`, "Exposable local domain").
    fn url_project(&self, local_domain: &str) -> Option<&str> {
        self.exposable_domains.get(local_domain).map(String::as_str)
    }

    fn project_of(&self, registration: &DomainRegistration) -> Option<&str> {
        if registration.source() != RegistrationSource::Config {
            return None;
        }
        self.domain_projects
            .get(&registration.display_pattern())
            .map(String::as_str)
    }

    pub fn project_root_for_registration(&self, registration: &DomainRegistration) -> Option<&str> {
        self.project_roots
            .get(&registration.display_pattern())
            .map(String::as_str)
    }

    pub fn record_access_for_registration(&self, registration: &DomainRegistration) {
        self.domain_accesses.record(registration.display_pattern());
    }

    pub fn last_accessed_at_for_registration(
        &self,
        registration: &DomainRegistration,
    ) -> Option<u64> {
        self.domain_accesses.get(&registration.display_pattern())
    }

    pub fn domain_access_log(&self) -> DomainAccessLog {
        self.domain_accesses.clone()
    }

    pub fn launch_sessions(&self) -> LaunchSessions {
        self.launch_sessions.clone()
    }
}

/// Shared, atomically-swappable runtime state. Lock-free reads.
pub type SharedState = Arc<ArcSwap<RuntimeState>>;

#[derive(Clone, Default)]
pub struct DomainAccessLog {
    entries: Arc<Mutex<HashMap<String, u64>>>,
}

impl DomainAccessLog {
    fn record(&self, pattern: String) {
        let Some(timestamp) = current_unix_millis() else {
            return;
        };

        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(pattern, timestamp);
        }
    }

    fn get(&self, pattern: &str) -> Option<u64> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(pattern).copied())
    }
}

fn current_unix_millis() -> Option<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(duration.as_millis()).ok()
}

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
    let public = request.extensions().get::<PublicSource>().cloned();
    let method = request.method().clone();
    let uri = request.uri().clone();
    let host_header = get_host(&request);
    let path = uri.path();
    // Public traffic reached the visitor over HTTPS, from their own address.
    let (proto, client_ip) = match &public {
        Some(public) => ("https", public.client_ip),
        None => (
            scheme.map(|Extension(s)| s.as_str()).unwrap_or("http"),
            client_addr.map(|Extension(a)| a.0),
        ),
    };
    // `source=public public_host=<host>` on the request log lines of public traffic.
    let log_source = public.as_ref().map(|_| field::display("public"));
    let log_public_host = public
        .as_ref()
        .map(|public| field::display(public.public_host.as_str()));

    // The public ingress refuses these first; the router does not rely on it.
    if public.is_some() && is_localify_path(path) {
        return public_error_response(StatusCode::NOT_FOUND);
    }

    if method == Method::GET && path == profile_indicator::SCRIPT_PATH {
        return profile_indicator::script_response();
    }

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
            info!(
                host = %host,
                source = log_source,
                public_host = log_public_host,
                "Domain not registered"
            );
            if public.is_some() {
                return public_error_response(StatusCode::NOT_FOUND);
            }
            build_not_registered_response(&host)
        }
        RouteResolution::NoRouteForPath {
            registration,
            host,
            path,
        } => {
            state.record_access_for_registration(registration);
            info!(
                host = %host,
                path = %path,
                source = log_source,
                public_host = log_public_host,
                "No route found"
            );
            if public.is_some() {
                return public_error_response(StatusCode::NOT_FOUND);
            }
            build_no_route_response(registration, &host, &path)
        }
        RouteResolution::Matched {
            registration,
            route,
        } => {
            state.record_access_for_registration(registration);
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
                    let launch_sessions = state.launch_sessions();
                    // Visitors get neither the indicator, whose script lives
                    // under `/.localify/`, nor the profile header: both are local detail.
                    let indicator = match public {
                        Some(_) => None,
                        None => browser_indicator(&request, target, &launch_sessions),
                    };
                    let mut request = request;
                    if indicator.is_some() {
                        profile_indicator::request_identity_encoding(&mut request);
                    }
                    let response = proxy_request(
                        target,
                        request,
                        host,
                        proto,
                        client_ip,
                        state.project_root_for_registration(registration),
                        public.as_ref(),
                    )
                    .await;
                    let response = match indicator {
                        Some(profile) => profile_indicator::inject(response, &profile),
                        None => response,
                    };
                    match public {
                        Some(_) => response,
                        None => with_launch_profile_header(response, target, &launch_sessions),
                    }
                }
            };

            info!(
                method = %method,
                host = %host,
                path = %path,
                status = response.status().as_u16(),
                source = log_source,
                public_host = log_public_host,
                "Request completed"
            );

            response
        }
    }
}

/// Whether a path is one of Localify's own endpoints rather than the app's.
pub(super) fn is_localify_path(path: &str) -> bool {
    path == LOCALIFY_PATH_PREFIX.trim_end_matches('/') || path.starts_with(LOCALIFY_PATH_PREFIX)
}

/// Profile to show in the page when this navigation reaches a service
/// running on a launch profile with the browser indicator on.
fn browser_indicator(
    request: &Request,
    target: &ProxyTarget,
    launch_sessions: &LaunchSessions,
) -> Option<String> {
    if !is_loopback_target(target) || !profile_indicator::is_page_navigation(request) {
        return None;
    }
    launch_sessions.indicator_for_port(target.port().value())
}

fn is_loopback_target(target: &ProxyTarget) -> bool {
    matches!(target.host(), "127.0.0.1" | "localhost")
}

/// Marks responses from a service launched with a launch profile, so
/// browser tools and API clients can tell which variables the service runs on.
fn with_launch_profile_header(
    mut response: Response,
    target: &ProxyTarget,
    launch_sessions: &LaunchSessions,
) -> Response {
    if !is_loopback_target(target) {
        return response;
    }
    let Some(profile) = launch_sessions.profile_for_port(target.port().value()) else {
        return response;
    };
    if let Ok(value) = HeaderValue::from_str(&profile) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(LAUNCH_PROFILE_HEADER), value);
    }
    response
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

    let _ = (state, target_query);
    action_error_response(
        StatusCode::BAD_REQUEST,
        "Open Localify to launch this service from the integrated terminal.",
    )
}

fn build_not_registered_response(domain: &str) -> Response {
    let domain = domain.split(':').next().unwrap_or(domain);
    let domain_raw = domain.trim_end_matches('.').to_lowercase();
    let domain = theme::html_escape(&domain_raw);
    let image_data_uri = embedded_assets::localify_error_data_uri();
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
    let image_data_uri = embedded_assets::localify_404_data_uri();
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
        // myapp.localify has len=2; no subdomain.
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
        let normalized = domain.replace(".localify", BRANDING.domain_suffix());
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
        let wildcard = reg("myapp.localify", true);
        let exact = reg("myapp.localify", false);
        let state = RuntimeState::new(vec![wildcard, exact]);

        let found = state.get_domain("myapp.localify").unwrap();
        // Exact patterns match before wildcards because both match,
        // but the exact match is returned since it's found first.
        assert!(!found.is_wildcard());
    }

    #[test]
    fn test_wildcard_matches_subdomain() {
        let wildcard = reg("myapp.localify", true);
        let state = RuntimeState::new(vec![wildcard]);

        let found = state.get_domain("blog.myapp.localify").unwrap();
        assert!(found.is_wildcard());
        assert_eq!(found.domain().as_str(), "myapp.localify");
    }

    #[test]
    fn runtime_state_returns_project_root_for_registration() {
        let registration = reg("app.localify", false);
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
    fn runtime_state_records_registration_access() {
        let state = RuntimeState::new(vec![reg("app.localify", false)]);
        let matched = state.get_domain("app.localify").unwrap();

        assert_eq!(state.last_accessed_at_for_registration(matched), None);

        state.record_access_for_registration(matched);

        assert!(state.last_accessed_at_for_registration(matched).is_some());
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
        let wildcard = reg("myapp.localify", true);
        let state = RuntimeState::new(vec![wildcard]);

        // This previously matched (bug DA3) — now fixed by DomainPattern
        assert!(state.get_domain("a.b.myapp.localify").is_none());
    }

    #[test]
    fn test_most_specific_wildcard_wins() {
        let broad = reg("myapp.localify", true);
        let specific = reg("sub.myapp.localify", true);
        let state = RuntimeState::new(vec![broad, specific]);

        let found = state.get_domain("blog.sub.myapp.localify").unwrap();
        assert_eq!(found.domain().as_str(), "sub.myapp.localify");
    }

    #[test]
    fn test_host_is_normalized_for_lookup() {
        let exact = reg("app.localify", false);
        let state = RuntimeState::new(vec![exact]);

        assert!(state.get_domain("APP.LOCALIFY:443").is_some());
    }

    // --- resolve_route tests ---

    #[test]
    fn resolve_route_missing_host() {
        let state = RuntimeState::new(vec![reg("app.localify", false)]);
        assert!(matches!(
            resolve_route(&state, None, "/"),
            RouteResolution::MissingHost
        ));
    }

    #[test]
    fn resolve_route_unknown_domain() {
        let state = RuntimeState::new(vec![reg("app.localify", false)]);
        match resolve_route(&state, Some("other.localify"), "/") {
            RouteResolution::DomainNotRegistered { host } => {
                assert_eq!(host, "other.localify");
            }
            other => panic!("expected DomainNotRegistered, got {:?}", other),
        }
    }

    #[test]
    fn resolve_route_matched() {
        let state = RuntimeState::new(vec![reg("app.localify", false)]);
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
        let state = RuntimeState::new(vec![reg("app.localify", false)]);
        assert!(matches!(
            resolve_route(&state, Some("app.localify:8080"), "/"),
            RouteResolution::Matched { .. }
        ));
    }

    // --- RuntimeState tests ---

    #[test]
    fn runtime_state_registrations_accessor() {
        let state = RuntimeState::new(vec![reg("b.localify", false), reg("a.localify", false)]);
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

        let original = RuntimeState::new(vec![reg("old.localify", false)]);
        let shared: SharedState = Arc::new(ArcSwap::from_pointee(original));

        // Verify original state
        assert!(shared.load().get_domain("old.localify").is_some());
        assert!(shared.load().get_domain("new.localify").is_none());

        // Swap to new state
        let replacement = Arc::new(RuntimeState::new(vec![reg("new.localify", false)]));
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

        let original = RuntimeState::new(vec![reg("old.localify", false)]);
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
        tx.send(vec![reg("new.localify", false)]).await.unwrap();
        handle.await.unwrap();

        assert!(shared.load().get_domain("old.localify").is_none());
        assert!(shared.load().get_domain("new.localify").is_some());
    }

    async fn html_upstream() -> (u16, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 8192];
            let read = stream.read(&mut buffer).await.unwrap();
            let _ = request_tx.send(String::from_utf8_lossy(&buffer[..read]).to_string());
            let body = "<!DOCTYPE html><html><head><title>App</title></head><body>hi</body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (port, request_rx)
    }

    #[tokio::test]
    async fn dangerous_launch_profile_pages_get_the_browser_indicator() {
        use std::sync::Arc;

        use arc_swap::ArcSwap;
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        use crate::daemon::launch_sessions::LaunchSessions;
        use crate::infrastructure::launch_sessions::{LaunchSessionDto, LaunchSessionUpdate};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (port, upstream_request) = html_upstream().await;
        let domain = DomainName::new(format!("app{}", BRANDING.domain_suffix())).unwrap();
        let registration = DomainRegistration::new(
            DomainPattern::Exact(domain.clone()),
            vec![Route::parse(&format!("/={port}")).unwrap()],
        );
        let sessions = LaunchSessions::default();
        sessions.apply(LaunchSessionUpdate::Running {
            session: LaunchSessionDto {
                id: "1".into(),
                project_id: "acme".into(),
                environment_slug: None,
                service_id: "web".into(),
                profile: "prod".into(),
                danger: true,
                target_port: port,
                started_at: 1,
                expires_at: None,
                sources: vec![],
                browser_indicator: true,
            },
        });
        let state: super::SharedState = Arc::new(ArcSwap::from_pointee(
            RuntimeState::new(vec![registration]).with_launch_sessions(sessions),
        ));

        let response = super::create_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::HOST, domain.as_str())
                    .header("sec-fetch-dest", "document")
                    .header(header::ACCEPT_ENCODING, "gzip, br")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.headers()["x-localify-profile"], "prod");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.starts_with(
            "<!DOCTYPE html><html><head><script src=\"/.localify/launch-profile.js\" data-profile=\"prod\" defer></script><title>App</title>"
        ));
        let upstream_request = upstream_request.await.unwrap().to_ascii_lowercase();
        assert!(!upstream_request.contains("accept-encoding"));

        let script = super::create_router(state)
            .oneshot(
                Request::builder()
                    .uri("/.localify/launch-profile.js")
                    .header(header::HOST, domain.as_str())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            script.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
    }

    // --- public traffic ---

    fn visitor() -> super::PublicSource {
        super::PublicSource {
            public_host: "journal-a8f3.localify.fyi".into(),
            client_ip: Some("203.0.113.7".parse().unwrap()),
        }
    }

    async fn send(
        state: super::SharedState,
        method: &str,
        host: &str,
        path: &str,
        public: Option<super::PublicSource>,
    ) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host)
            .header(header::ORIGIN, format!("http://{host}"))
            .body(Body::empty())
            .unwrap();
        if let Some(public) = public {
            request.extensions_mut().insert(public);
        }
        let response = super::create_router(state)
            .layer(axum::Extension(super::ClientAddr(
                std::net::Ipv4Addr::LOCALHOST.into(),
            )))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, String::from_utf8_lossy(&body).to_string())
    }

    fn shared(registrations: Vec<DomainRegistration>) -> super::SharedState {
        std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(RuntimeState::new(
            registrations,
        )))
    }

    #[tokio::test]
    async fn public_traffic_never_reaches_localify_endpoints() {
        let state = shared(vec![reg("app.localify", false)]);
        for (method, path) in [
            ("GET", "/.localify/launch-profile.js"),
            ("POST", "/.localify/open-in-editor"),
            ("POST", "/.localify/run-dev-server"),
        ] {
            let host = format!("app{}", BRANDING.domain_suffix());
            let (status, _, body) = send(state.clone(), method, &host, path, Some(visitor())).await;
            assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{path}");
            assert!(body.contains("Nothing is served at this address"), "{path}");
        }

        // Local traffic still reaches them.
        let host = format!("app{}", BRANDING.domain_suffix());
        let (status, _, _) = send(state, "GET", &host, "/.localify/launch-profile.js", None).await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn public_errors_are_generic_and_local_errors_keep_their_help() {
        let domain = DomainName::new(format!("app{}", BRANDING.domain_suffix())).unwrap();
        let registration = DomainRegistration::new(
            DomainPattern::Exact(domain.clone()),
            vec![Route::parse("/api=3000").unwrap()],
        );
        let state = shared(vec![registration]);
        let missing = format!("missing{}", BRANDING.domain_suffix());

        for (host, path) in [(missing.as_str(), "/"), (domain.as_str(), "/other")] {
            let (status, _, body) = send(state.clone(), "GET", host, path, Some(visitor())).await;
            assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
            assert!(
                !body.contains(&format!("{} ", BRANDING.cli_command())),
                "{body}"
            );
            assert!(!body.contains(host), "{body}");

            let (status, _, body) = send(state.clone(), "GET", host, path, None).await;
            assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
            assert!(
                body.contains(&format!("{} ", BRANDING.cli_command())),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn public_request_log_lines_name_their_source() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_target(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // Tests on other threads may have cached these callsites as disabled.
        tracing::callsite::rebuild_interest_cache();
        let state = shared(vec![]);

        send(
            state.clone(),
            "GET",
            "missing.localify",
            "/",
            Some(visitor()),
        )
        .await;
        send(state, "GET", "local.localify", "/", None).await;

        let output = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2, "{output}");
        assert!(
            lines[0].ends_with(
                "Domain not registered host=missing.localify source=public public_host=journal-a8f3.localify.fyi"
            ),
            "{output}"
        );
        assert!(
            lines[1].ends_with("Domain not registered host=local.localify"),
            "{output}"
        );
    }

    #[tokio::test]
    async fn visitors_get_no_launch_profile_indicator_or_header() {
        use std::sync::Arc;

        use arc_swap::ArcSwap;

        use crate::daemon::launch_sessions::LaunchSessions;
        use crate::infrastructure::launch_sessions::{LaunchSessionDto, LaunchSessionUpdate};

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let (port, upstream_request) = html_upstream().await;
        let domain = DomainName::new(format!("app{}", BRANDING.domain_suffix())).unwrap();
        let registration = DomainRegistration::new(
            DomainPattern::Exact(domain.clone()),
            vec![Route::parse(&format!("/={port}")).unwrap()],
        );
        let sessions = LaunchSessions::default();
        sessions.apply(LaunchSessionUpdate::Running {
            session: LaunchSessionDto {
                id: "1".into(),
                project_id: "acme".into(),
                environment_slug: None,
                service_id: "web".into(),
                profile: "prod".into(),
                danger: true,
                target_port: port,
                started_at: 1,
                expires_at: None,
                sources: vec![],
                browser_indicator: true,
            },
        });
        let state: super::SharedState = Arc::new(ArcSwap::from_pointee(
            RuntimeState::new(vec![registration]).with_launch_sessions(sessions),
        ));

        let response = {
            use axum::body::Body;
            use axum::http::Request;
            use tower::ServiceExt;
            let mut request = Request::builder()
                .uri("/")
                .header(header::HOST, domain.as_str())
                .header("sec-fetch-dest", "document")
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(visitor());
            super::create_router(state).oneshot(request).await.unwrap()
        };

        assert!(response.headers().get("x-localify-profile").is_none());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("launch-profile.js"));
        let upstream_request = upstream_request.await.unwrap().to_ascii_lowercase();
        assert!(upstream_request.contains("x-forwarded-host: journal-a8f3.localify.fyi"));
        assert!(upstream_request.contains("x-forwarded-for: 203.0.113.7"));
    }
}
