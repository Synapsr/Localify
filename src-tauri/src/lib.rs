mod commands;
mod remote;
mod terminal;

use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::io::Write;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use commands::{AppState, DeepLinkLaunchDto, DeepLinkPayloadDto, DeepLinkRouteDto};
use localify::branding::BRANDING;
use localify::desktop::DesktopApi;
use localify::desktop::dto::{
    DomainInfoDto, OpenInEditorRequest, ProjectInfoDto, StartDaemonRequest,
};
use localify::desktop::run_pty::AppInstallation;
use localify::desktop::terminal::TerminalManager;
use localify::infrastructure::running_services::RunningServiceStore;
use localify::infrastructure::shell_env;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
#[cfg(target_os = "macos")]
use tauri::{async_runtime, menu::AboutMetadata};
#[cfg(desktop)]
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_deep_link::DeepLinkExt;
#[cfg(target_os = "macos")]
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

#[cfg(desktop)]
const AUTOSTART_ARG: &str = "--autostart";
const TRAY_ID: &str = "localify-tray";
const TRAY_OPEN: &str = "tray-open";
const TRAY_START: &str = "tray-start";
const TRAY_STOP: &str = "tray-stop";
const TRAY_RESTART: &str = "tray-restart";
const TRAY_QUIT: &str = "tray-quit";
const TRAY_DOMAIN_COPY_PREFIX: &str = "tray-domain-copy:";
const TRAY_DOMAIN_OPEN_PREFIX: &str = "tray-domain-open:";
const TRAY_PROJECT_LIMIT: usize = 10;
#[cfg(target_os = "macos")]
const MENU_CHECK_UPDATES: &str = "menu-check-updates";
const TRAY_TEMPLATE_ICON: &[u8] = include_bytes!("../icons/tray-template.png");
const UPDATER_PUBLIC_KEY: &str = include_str!("../updater.pub");
const UPDATER_ENDPOINT: Option<&str> = option_env!("LOCALIFY_UPDATER_ENDPOINT");
const DEEP_LINK_EVENT: &str = "localify://deep-link";
const LOCALIFY_SCHEME: &str = "localify";

#[derive(Default)]
struct TrayState {
    quitting: AtomicBool,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct UpdateMenuState {
    checking: AtomicBool,
}

fn default_config_path() -> PathBuf {
    std::env::var_os(BRANDING.env_var("CONFIG_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|| BRANDING.config_path())
}

fn default_binary_path() -> PathBuf {
    if let Some(path) = std::env::var_os(BRANDING.env_var("CLI_PATH")) {
        return PathBuf::from(path);
    }

    if cfg!(debug_assertions)
        && let Some(path) = workspace_binary_path("debug")
    {
        return path;
    }

    if let Some(path) = sibling_binary_path() {
        return path;
    }

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let binary_name = if cfg!(target_os = "windows") {
        "localify.exe"
    } else {
        BRANDING.cli_command()
    };

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join(profile)
        .join(binary_name)
}

fn workspace_binary_path(profile: &str) -> Option<PathBuf> {
    let binary_name = if cfg!(target_os = "windows") {
        "localify.exe"
    } else {
        BRANDING.cli_command()
    };

    let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join(profile)
        .join(binary_name);

    candidate.exists().then_some(candidate)
}

fn sibling_binary_path() -> Option<PathBuf> {
    let binary_name = if cfg!(target_os = "windows") {
        "localify.exe"
    } else {
        BRANDING.cli_command()
    };

    let candidate = std::env::current_exe().ok()?.parent()?.join(binary_name);
    candidate.exists().then_some(candidate)
}

pub(crate) fn updater_endpoint() -> Option<&'static str> {
    if cfg!(debug_assertions) {
        return None;
    }

    UPDATER_ENDPOINT
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn updater_public_key() -> &'static str {
    UPDATER_PUBLIC_KEY.trim()
}

pub(crate) fn configure_updater_builder(
    builder: tauri_plugin_updater::UpdaterBuilder,
) -> tauri_plugin_updater::UpdaterBuilder {
    // Updater archives must be read byte-for-byte; let the signed .tar.gz stay opaque.
    builder.configure_client(|client| client.no_gzip().no_brotli().no_zstd().no_deflate())
}

fn runtime_desktop_name() -> String {
    BRANDING.runtime_desktop_name()
}

fn open_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };

    window.show()?;
    let _ = window.unminimize();
    window.set_focus()?;
    Ok(())
}

#[cfg(desktop)]
fn argv_has_autostart_arg(argv: &[String]) -> bool {
    argv.iter().any(|arg| arg == AUTOSTART_ARG)
}

#[cfg(desktop)]
fn launched_from_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

#[cfg(not(desktop))]
fn launched_from_autostart() -> bool {
    false
}

#[cfg(desktop)]
pub(crate) fn autostart_enabled<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[cfg(not(desktop))]
pub(crate) fn autostart_enabled<R: Runtime>(_app: &AppHandle<R>) -> Result<bool, String> {
    Ok(false)
}

#[cfg(desktop)]
pub(crate) fn enable_autostart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if autostart_enabled(app)? {
        return Ok(());
    }

    app.autolaunch().enable().map_err(|error| error.to_string())
}

#[cfg(not(desktop))]
pub(crate) fn enable_autostart<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    Ok(())
}

#[cfg(desktop)]
pub(crate) fn disable_autostart<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if !autostart_enabled(app)? {
        return Ok(());
    }

    app.autolaunch()
        .disable()
        .map_err(|error| error.to_string())
}

#[cfg(not(desktop))]
pub(crate) fn disable_autostart<R: Runtime>(_app: &AppHandle<R>) -> Result<(), String> {
    Ok(())
}

#[derive(Debug)]
enum DeepLinkAction {
    Projects,
    Logs { domain: String },
    InEditor { domain: String },
    RunLocalDevServer { domain: String },
    LaunchService(DeepLinkLaunchDto),
}

#[derive(Debug, Clone, Copy)]
enum DeepLinkDelivery {
    Pending,
    Emit,
}

#[derive(Debug)]
struct ResolvedDomainProject {
    project_id: Option<String>,
    root_path: Option<String>,
}

fn handle_startup_deep_links<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(urls) = app
        .deep_link()
        .get_current()
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };

    handle_deep_link_urls(app, &urls, DeepLinkDelivery::Pending);
    Ok(())
}

fn handle_deep_link_urls<R: Runtime>(
    app: &AppHandle<R>,
    urls: &[tauri::Url],
    delivery: DeepLinkDelivery,
) {
    for url in urls {
        if let Err(error) = handle_deep_link_url(app, url, delivery) {
            eprintln!("failed to handle deep link '{}': {error}", url);
        }
    }
}

fn handle_deep_link_url<R: Runtime>(
    app: &AppHandle<R>,
    url: &tauri::Url,
    delivery: DeepLinkDelivery,
) -> Result<(), String> {
    let action = parse_deep_link_action(url)?;
    open_main_window(app).map_err(|error| error.to_string())?;
    execute_deep_link_action(app, action, delivery)
}

fn parse_deep_link_action(url: &tauri::Url) -> Result<DeepLinkAction, String> {
    if url.scheme() != LOCALIFY_SCHEME {
        return Err(format!("Unsupported deep-link scheme '{}'.", url.scheme()));
    }

    let host = url
        .host_str()
        .ok_or_else(|| "Deep link is missing an action namespace.".to_string())?;

    match (host, url.path()) {
        // License emails already sent link to the settings screen that no longer exists.
        ("open", "/projects" | "/settings/licensing") => Ok(DeepLinkAction::Projects),
        ("open", "/logs") => Ok(DeepLinkAction::Logs {
            domain: query_param(url, "domain")?,
        }),
        ("action", "/open-in-editor") => Ok(DeepLinkAction::InEditor {
            domain: query_param(url, "domain")?,
        }),
        ("action", "/run-local-dev-server") => Ok(DeepLinkAction::RunLocalDevServer {
            domain: query_param(url, "domain")?,
        }),
        ("action", "/launch-service") => Ok(DeepLinkAction::LaunchService(DeepLinkLaunchDto {
            project_id: query_param(url, "project")?,
            environment_slug: query_param(url, "env")?,
            service_id: query_param(url, "service")?,
            profile: query_param(url, "profile").ok(),
        })),
        _ => Err(format!(
            "Unsupported deep-link target '{}{}'.",
            host,
            url.path()
        )),
    }
}

fn query_param(url: &tauri::Url, key: &str) -> Result<String, String> {
    url.query_pairs()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.into_owned()))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Deep link is missing query parameter '{}'.", key))
}

fn execute_deep_link_action<R: Runtime>(
    app: &AppHandle<R>,
    action: DeepLinkAction,
    delivery: DeepLinkDelivery,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    match action {
        // The terminal panel starts it, as its launch button does, so the
        // session shows from its first line.
        DeepLinkAction::LaunchService(launch) => deliver_deep_link_payload(
            app,
            DeepLinkPayloadDto {
                route: DeepLinkRouteDto::LaunchService,
                project_id: Some(launch.project_id.clone()),
                view: None,
                launch: Some(launch),
            },
            delivery,
        ),
        DeepLinkAction::Projects => deliver_deep_link_payload(
            app,
            DeepLinkPayloadDto {
                route: DeepLinkRouteDto::Projects,
                project_id: None,
                view: None,
                launch: None,
            },
            delivery,
        ),
        DeepLinkAction::Logs { domain } => {
            let resolved = resolve_domain_project(&state.api, &domain)?;
            let payload = match resolved.and_then(|resolved| resolved.project_id) {
                Some(project_id) => DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Logs,
                    project_id: Some(project_id),
                    view: Some("requests".into()),
                    launch: None,
                },
                None => DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
                    launch: None,
                },
            };

            deliver_deep_link_payload(app, payload, delivery)
        }
        DeepLinkAction::InEditor { domain } => {
            let resolved = resolve_domain_project(&state.api, &domain)?;
            let payload = resolved
                .as_ref()
                .and_then(|resolved| resolved.project_id.as_ref())
                .map(|project_id| DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Logs,
                    project_id: Some(project_id.clone()),
                    view: Some("requests".into()),
                    launch: None,
                })
                .unwrap_or(DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
                    launch: None,
                });

            deliver_deep_link_payload(app, payload, delivery)?;

            let Some(resolved) = resolved else {
                return Err(format!(
                    "No Localify project is linked to domain '{}'.",
                    normalize_domain(&domain)
                ));
            };

            let Some(root_path) = resolved.root_path else {
                return Err(format!(
                    "No local project folder is linked to domain '{}'.",
                    normalize_domain(&domain)
                ));
            };

            state
                .api
                .open_in_editor(OpenInEditorRequest { path: root_path })
                .map_err(|error| error.to_string())
        }
        DeepLinkAction::RunLocalDevServer { domain } => {
            let resolved = resolve_domain_project(&state.api, &domain)?;
            let payload = resolved
                .as_ref()
                .and_then(|resolved| resolved.project_id.as_ref())
                .map(|project_id| DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Logs,
                    project_id: Some(project_id.clone()),
                    view: Some("requests".into()),
                    launch: None,
                })
                .unwrap_or(DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
                    launch: None,
                });

            deliver_deep_link_payload(app, payload, delivery)?;

            if resolved.is_none() {
                return Err(format!(
                    "No Localify project is linked to domain '{}'.",
                    normalize_domain(&domain)
                ));
            }

            Ok(())
        }
    }
}

fn deliver_deep_link_payload<R: Runtime>(
    app: &AppHandle<R>,
    payload: DeepLinkPayloadDto,
    delivery: DeepLinkDelivery,
) -> Result<(), String> {
    match delivery {
        DeepLinkDelivery::Pending => {
            app.state::<AppState>().set_pending_deep_link(payload);
            Ok(())
        }
        DeepLinkDelivery::Emit => app
            .emit(DEEP_LINK_EVENT, payload)
            .map_err(|error| error.to_string()),
    }
}

fn resolve_domain_project(
    api: &DesktopApi,
    domain: &str,
) -> Result<Option<ResolvedDomainProject>, String> {
    let domain = normalize_domain(domain);
    let bootstrap = api.bootstrap().map_err(|error| error.to_string())?;

    let Some(matched_domain) = bootstrap
        .domains
        .iter()
        .filter(|candidate| domain_matches_registration(&domain, candidate))
        .max_by_key(|candidate| {
            (
                candidate.registration.domain.len(),
                !candidate.registration.wildcard,
            )
        })
    else {
        return Ok(None);
    };

    let project_id = matched_domain.project_id.clone();
    let root_path = project_id
        .as_deref()
        .and_then(|project_id| project_root_path(&bootstrap.projects, project_id));
    Ok(Some(ResolvedDomainProject {
        project_id,
        root_path,
    }))
}

fn normalize_domain(domain: &str) -> String {
    domain
        .split(':')
        .next()
        .unwrap_or(domain)
        .trim()
        .trim_end_matches('.')
        .to_lowercase()
}

fn domain_matches_registration(domain: &str, candidate: &DomainInfoDto) -> bool {
    let registered = normalize_domain(&candidate.registration.domain);
    if candidate.registration.wildcard {
        domain == registered || domain.ends_with(&format!(".{registered}"))
    } else {
        domain == registered
    }
}

fn project_root_path(projects: &[ProjectInfoDto], project_id: &str) -> Option<String> {
    projects
        .iter()
        .find(|project| project.id == project_id)
        .and_then(|project| project.root_path.clone())
}

fn daemon_running<R: Runtime>(app: &AppHandle<R>) -> bool {
    let state = app.state::<AppState>();
    state
        .api
        .daemon_status()
        .map(|status| status.running)
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn update_check_running<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.state::<UpdateMenuState>()
        .checking
        .load(Ordering::Relaxed)
}

fn build_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let running = daemon_running(app);
    let toggle_id = if running { TRAY_STOP } else { TRAY_START };
    let toggle_label = if running {
        "Stop daemon"
    } else {
        "Start daemon"
    };

    let open = MenuItem::with_id(app, TRAY_OPEN, "Open Localify", true, None::<&str>)?;
    let top_separator = PredefinedMenuItem::separator(app)?;
    let toggle = MenuItem::with_id(app, toggle_id, toggle_label, true, None::<&str>)?;
    let restart = MenuItem::with_id(app, TRAY_RESTART, "Restart daemon", running, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let bottom_separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "Quit Localify", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open, &top_separator])?;
    append_tray_project_items(app, &menu)?;
    menu.append(&separator)?;
    menu.append(&toggle)?;
    menu.append(&restart)?;
    menu.append(&bottom_separator)?;
    menu.append(&quit)?;
    Ok(menu)
}

fn append_tray_project_items<R: Runtime>(app: &AppHandle<R>, menu: &Menu<R>) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let projects = tray_visible_projects(state.api.list_projects().unwrap_or_default());
    let domains = state.api.list_domains().unwrap_or_default();
    let domains_by_project = tray_domains_by_project(domains);

    if projects.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "tray-projects-empty",
            "No projects",
            false,
            None::<&str>,
        )?)?;
        return Ok(());
    }

    for project in projects {
        let project_menu = build_tray_project_menu(app, &project, &domains_by_project)?;
        menu.append(&project_menu)?;
    }

    Ok(())
}

fn build_tray_project_menu<R: Runtime>(
    app: &AppHandle<R>,
    project: &ProjectInfoDto,
    domains_by_project: &HashMap<String, Vec<DomainInfoDto>>,
) -> tauri::Result<Submenu<R>> {
    let project_menu = Submenu::new(app, &project.name, project.enabled)?;
    let domains = domains_by_project
        .get(&project.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    if domains.is_empty() {
        project_menu.append(&MenuItem::with_id(
            app,
            format!("tray-project-empty:{}", project.id),
            "No domains",
            false,
            None::<&str>,
        )?)?;
        return Ok(project_menu);
    }

    for domain in domains {
        let domain_menu = build_tray_domain_menu(app, domain)?;
        project_menu.append(&domain_menu)?;
    }

    Ok(project_menu)
}

fn build_tray_domain_menu<R: Runtime>(
    app: &AppHandle<R>,
    domain: &DomainInfoDto,
) -> tauri::Result<Submenu<R>> {
    let domain_name = &domain.registration.domain;
    let domain_menu = Submenu::new(app, domain_name, domain.project_enabled)?;
    domain_menu.append(&MenuItem::with_id(
        app,
        format!("{TRAY_DOMAIN_COPY_PREFIX}{domain_name}"),
        "Copy URL",
        true,
        None::<&str>,
    )?)?;
    domain_menu.append(&MenuItem::with_id(
        app,
        format!("{TRAY_DOMAIN_OPEN_PREFIX}{domain_name}"),
        "Open in Browser",
        true,
        None::<&str>,
    )?)?;
    Ok(domain_menu)
}

fn tray_visible_projects(mut projects: Vec<ProjectInfoDto>) -> Vec<ProjectInfoDto> {
    projects.sort_by(|left, right| {
        right
            .enabled
            .cmp(&left.enabled)
            .then_with(|| right.has_reachable_domain.cmp(&left.has_reachable_domain))
            .then_with(|| right.domain_count.cmp(&left.domain_count))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    projects.truncate(TRAY_PROJECT_LIMIT);
    projects
}

fn tray_domains_by_project(domains: Vec<DomainInfoDto>) -> HashMap<String, Vec<DomainInfoDto>> {
    let mut grouped = HashMap::<String, Vec<DomainInfoDto>>::new();
    for domain in domains {
        let Some(project_id) = domain.project_id.clone() else {
            continue;
        };
        grouped.entry(project_id).or_default().push(domain);
    }

    for domains in grouped.values_mut() {
        domains.sort_by(|left, right| left.registration.domain.cmp(&right.registration.domain));
    }

    grouped
}

pub(crate) fn sync_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    tray.set_menu(Some(build_tray_menu(app)?))?;
    Ok(())
}

fn run_tray_action<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    if let Some(domain) = id.strip_prefix(TRAY_DOMAIN_COPY_PREFIX) {
        return copy_tray_domain_url(app, domain);
    }

    if let Some(domain) = id.strip_prefix(TRAY_DOMAIN_OPEN_PREFIX) {
        return open_tray_domain_url(app, domain);
    }

    match id {
        TRAY_OPEN => open_main_window(app).map_err(|error| error.to_string()),
        TRAY_START => {
            let state = app.state::<AppState>();
            state
                .api
                .start_daemon(StartDaemonRequest { verbose: false })
                .map(|_| ())
                .map_err(|error| error.to_string())?;
            sync_tray_menu(app).map_err(|error| error.to_string())
        }
        TRAY_STOP => {
            let state = app.state::<AppState>();
            state
                .api
                .stop_daemon()
                .map(|_| ())
                .map_err(|error| error.to_string())?;
            sync_tray_menu(app).map_err(|error| error.to_string())
        }
        TRAY_RESTART => {
            let state = app.state::<AppState>();
            state
                .api
                .restart_daemon(StartDaemonRequest { verbose: false })
                .map(|_| ())
                .map_err(|error| error.to_string())?;
            sync_tray_menu(app).map_err(|error| error.to_string())
        }
        TRAY_QUIT => {
            let tray_state = app.state::<TrayState>();
            tray_state.quitting.store(true, Ordering::Relaxed);
            app.exit(0);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_tray_action_error<R: Runtime>(app: &AppHandle<R>, id: &str, error: String) {
    eprintln!("tray action '{id}' failed: {error}");

    #[cfg(not(target_os = "macos"))]
    let _ = app;

    #[cfg(target_os = "macos")]
    show_message_dialog(
        app,
        "Localify Action Failed",
        error,
        MessageDialogKind::Error,
    );
}

fn copy_tray_domain_url<R: Runtime>(app: &AppHandle<R>, domain: &str) -> Result<(), String> {
    let url = tray_domain_url(app, domain)?;
    copy_text_to_clipboard(&url)
}

fn open_tray_domain_url<R: Runtime>(app: &AppHandle<R>, domain: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let url = tray_domain_url(app, domain)?;
    state
        .api
        .open_project_url_with_preferred_browser(&url)
        .map_err(|error| error.to_string())
}

fn tray_domain_url<R: Runtime>(app: &AppHandle<R>, domain: &str) -> Result<String, String> {
    let state = app.state::<AppState>();
    let domain = find_tray_domain(&state.api, domain)?
        .ok_or_else(|| format!("No Localify domain named '{domain}' was found."))?;
    let status = state
        .api
        .daemon_status()
        .map_err(|error| error.to_string())?;
    Ok(build_domain_url(
        &domain,
        status.http_port,
        status.https_port,
    ))
}

fn find_tray_domain(api: &DesktopApi, domain: &str) -> Result<Option<DomainInfoDto>, String> {
    Ok(api
        .list_domains()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|item| item.registration.domain == domain))
}

fn build_domain_url(domain: &DomainInfoDto, http_port: u16, https_port: u16) -> String {
    let scheme = if domain.registration.https_enabled {
        "https"
    } else {
        "http"
    };
    let port = if domain.registration.https_enabled {
        https_port
    } else {
        http_port
    };

    format!(
        "{scheme}://{}{}",
        domain.registration.domain,
        port_segment(scheme, port)
    )
}

fn port_segment(scheme: &str, port: u16) -> String {
    if (scheme == "http" && port == 80) || (scheme == "https" && port == 443) {
        String::new()
    } else {
        format!(":{port}")
    }
}

#[cfg(target_os = "macos")]
fn copy_text_to_clipboard(value: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to start pbcopy: {error}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err("Failed to open pbcopy stdin.".into());
    };

    stdin
        .write_all(value.as_bytes())
        .map_err(|error| format!("Failed to write to pbcopy: {error}"))?;
    drop(stdin);

    let status = child
        .wait()
        .map_err(|error| format!("Failed to wait for pbcopy: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pbcopy exited with status {status}."))
    }
}

#[cfg(not(target_os = "macos"))]
fn copy_text_to_clipboard(_value: &str) -> Result<(), String> {
    Err("Tray URL copying is currently only implemented on macOS.".into())
}

#[cfg(target_os = "macos")]
fn build_about_metadata<R: Runtime>(app: &AppHandle<R>) -> AboutMetadata<'static> {
    AboutMetadata {
        name: Some(BRANDING.app_name.into()),
        version: Some(app.package_info().version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn build_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let checking = update_check_running(app);
    let update_label = if checking {
        "Checking for Updates…"
    } else {
        "Check for Updates…"
    };

    let check_updates = MenuItem::with_id(
        app,
        MENU_CHECK_UPDATES,
        update_label,
        !checking,
        None::<&str>,
    )?;
    let app_menu = Submenu::with_items(
        app,
        BRANDING.app_name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(build_about_metadata(app)))?,
            &PredefinedMenuItem::separator(app)?,
            &check_updates,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])
}

#[cfg(target_os = "macos")]
fn sync_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    app.set_menu(build_app_menu(app)?)?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sync_app_menu<R: Runtime>(_app: &AppHandle<R>) -> tauri::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_message_dialog<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    message: String,
    kind: MessageDialogKind,
) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(kind)
        .buttons(MessageDialogButtons::Ok)
        .blocking_show();
}

#[cfg(target_os = "macos")]
fn confirm_update_install<R: Runtime>(
    app: &AppHandle<R>,
    version: &str,
    current_version: &str,
) -> bool {
    let message = format!(
        "Version {version} is available.\n\nCurrent version: {current_version}\n\nInstall the update now? Localify will restart automatically when it finishes."
    );

    app.dialog()
        .message(message)
        .title("Update Available")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install".into(),
            "Not now".into(),
        ))
        .blocking_show()
}

#[cfg(target_os = "macos")]
fn finish_update_check<R: Runtime>(app: &AppHandle<R>) {
    app.state::<UpdateMenuState>()
        .checking
        .store(false, Ordering::Relaxed);
    let _ = sync_app_menu(app);
}

#[cfg(target_os = "macos")]
async fn run_manual_update_check<R: Runtime>(app: AppHandle<R>) {
    let result = run_manual_update_check_inner(app.clone()).await;
    if let Err(error) = result {
        show_message_dialog(&app, "Update Check Failed", error, MessageDialogKind::Error);
        finish_update_check(&app);
    }
}

#[cfg(target_os = "macos")]
async fn run_manual_update_check_inner<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let current_version = app.package_info().version.to_string();
    let update_check = commands::check_app_update_with_handle(app.clone()).await?;

    if !update_check.configured {
        show_message_dialog(
            &app,
            "Updates Unavailable",
            "Desktop updates are not configured for this build.".into(),
            MessageDialogKind::Info,
        );
        finish_update_check(&app);
        return Ok(());
    }

    let Some(update) = update_check.update else {
        show_message_dialog(
            &app,
            "You're Up to Date",
            format!("Localify {current_version} is already the latest version."),
            MessageDialogKind::Info,
        );
        finish_update_check(&app);
        return Ok(());
    };

    if !confirm_update_install(&app, &update.version, &current_version) {
        finish_update_check(&app);
        return Ok(());
    }

    commands::install_app_update_with_handle(app).await
}

#[cfg(target_os = "macos")]
fn run_app_menu_action<R: Runtime + 'static>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
    if id != MENU_CHECK_UPDATES {
        return Ok(());
    }

    if update_check_running(app) {
        let app = app.clone();
        async_runtime::spawn(async move {
            show_message_dialog(
                &app,
                "Update Check In Progress",
                "Localify is already checking for an update.".into(),
                MessageDialogKind::Info,
            );
        });
        return Ok(());
    }

    app.state::<UpdateMenuState>()
        .checking
        .store(true, Ordering::Relaxed);
    if let Err(error) = sync_app_menu(app) {
        finish_update_check(app);
        return Err(error.to_string());
    }

    let app = app.clone();
    async_runtime::spawn(async move {
        run_manual_update_check(app).await;
    });

    Ok(())
}

fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(runtime_desktop_name())
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if let Err(error) = run_tray_action(app, id) {
                handle_tray_action_error(app, id, error);
            }
        });

    #[cfg(target_os = "macos")]
    {
        tray = tray
            .icon(Image::from_bytes(TRAY_TEMPLATE_ICON)?)
            .icon_as_template(true);
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }

    tray.build(app)?;
    Ok(())
}

fn setup_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    sync_app_menu(app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let api = DesktopApi::new(default_config_path(), default_binary_path());

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !argv_has_autostart_arg(&argv) {
                let _ = open_main_window(app);
            }
        }));
        builder = builder.plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ));
    }

    let executable = api.binary_path().to_path_buf();
    let builder = builder
        .on_menu_event(|app, event| {
            #[cfg(target_os = "macos")]
            if let Err(error) = run_app_menu_action(app, event.id().as_ref()) {
                eprintln!("app menu action '{}' failed: {error}", event.id().as_ref());
            }

            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        })
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::with_api(api))
        .manage(remote::RemoteStreams::default())
        .manage(TrayState::default());

    #[cfg(target_os = "macos")]
    let builder = builder.manage(UpdateMenuState::default());

    builder
        .setup(move |app| {
            // In setup: the panel's events need the app's handle.
            let terminal = TerminalManager::new(
                executable.clone(),
                shell_env::user_shell(),
                RunningServiceStore::new_default()?,
                AppInstallation::of_this_process(),
                Arc::new(terminal::WindowEvents(app.handle().clone())),
            );
            terminal.warm_shell_environment();
            app.manage(terminal);
            app.handle().plugin(
                tauri_plugin_updater::Builder::new()
                    .pubkey(updater_public_key())
                    .build(),
            )?;
            let app_handle = app.handle().clone();
            app_handle.clone().deep_link().on_open_url(move |event| {
                let urls = event.urls();
                handle_deep_link_urls(&app_handle, &urls, DeepLinkDelivery::Emit);
            });
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&runtime_desktop_name());
            }
            setup_app_menu(app.handle())?;
            setup_tray(app.handle())?;
            if !launched_from_autostart() {
                open_main_window(app.handle())?;
            }
            if let Err(error) = handle_startup_deep_links(app.handle()) {
                eprintln!("failed to process startup deep links: {error}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }

            if let WindowEvent::CloseRequested { api, .. } = event {
                let tray_state = window.app_handle().state::<TrayState>();
                if tray_state.quitting.load(Ordering::Relaxed) {
                    return;
                }

                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap,
            commands::get_autostart_status,
            commands::set_autostart_enabled,
            commands::check_app_update,
            commands::install_app_update,
            commands::read_install_attribution_clipboard,
            commands::get_config,
            commands::update_config,
            commands::export_settings,
            commands::preview_settings_import,
            commands::import_settings,
            commands::get_daemon_status,
            commands::list_domains,
            commands::list_projects,
            commands::get_available_port,
            commands::detect_project_init_plan,
            commands::install_localify,
            commands::setup_cli_access,
            commands::setup_node_trust,
            commands::trust_ca,
            commands::trust_ios_simulators,
            commands::get_agent_mcp_status,
            commands::install_agent_mcp,
            commands::connect_agents_once,
            commands::get_cloud_account,
            commands::start_cloud_sign_in,
            commands::poll_cloud_sign_in,
            commands::sign_out_cloud,
            commands::get_cloud_plan,
            commands::start_cloud_checkout,
            commands::open_cloud_billing_portal,
            remote::get_remote_status,
            remote::get_remote_machines,
            remote::change_remote_settings,
            remote::answer_remote_pairing,
            remote::unpair_remote_machine,
            remote::remove_remote_machine,
            remote::open_remote_stream,
            remote::close_remote_stream,
            commands::refresh_public_tunnel,
            commands::list_public_urls,
            commands::create_public_url,
            commands::suggest_public_url_slug,
            commands::check_public_url_slug,
            commands::remove_public_url,
            commands::set_public_url_protection,
            commands::claim_public_url,
            commands::list_port_conflicts,
            commands::kill_port_conflict,
            commands::get_uninstall_preview,
            commands::uninstall_localify,
            commands::create_project,
            commands::create_project_with_domains,
            commands::init_project_from_folder,
            commands::add_project_service,
            commands::update_project,
            commands::relink_project_root,
            commands::remove_project,
            commands::remove_project_environment,
            commands::discover_project_worktrees,
            commands::attach_project_worktree,
            commands::assign_domain_project,
            commands::register_domain,
            commands::update_domain,
            commands::unregister_domain,
            commands::add_route,
            commands::remove_route,
            commands::list_routes,
            commands::start_daemon,
            commands::stop_daemon,
            commands::restart_daemon,
            commands::reload_daemon,
            commands::read_logs,
            commands::open_in_editor,
            commands::open_env_in_editor,
            commands::read_environment_file,
            commands::list_environment_files,
            commands::save_environment_file,
            commands::preview_environment_file_copy,
            commands::copy_environment_files,
            commands::open_project_url,
            commands::run_local_dev_server,
            commands::stop_local_dev_server,
            commands::update_launcher_preferences,
            commands::terminal_list_sessions,
            commands::terminal_create_session,
            commands::terminal_launch_service,
            commands::terminal_restart_service,
            commands::terminal_stop_service,
            commands::terminal_write,
            commands::terminal_resize,
            commands::terminal_close,
            commands::stop_service_run,
            commands::shell_environment_status,
            commands::reload_shell_environment,
            commands::consume_pending_deep_link
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn license_email_deep_link_opens_projects() {
        let url = tauri::Url::parse("localify://open/settings/licensing").unwrap();

        assert!(matches!(
            parse_deep_link_action(&url),
            Ok(DeepLinkAction::Projects)
        ));
    }

    #[test]
    fn launch_service_deep_link_names_the_service() {
        let url = tauri::Url::parse(
            "localify://action/launch-service?project=my%20app&env=feat-auth&service=web",
        )
        .unwrap();

        let Ok(DeepLinkAction::LaunchService(launch)) = parse_deep_link_action(&url) else {
            panic!("not a launch-service action");
        };
        assert_eq!(
            launch,
            DeepLinkLaunchDto {
                project_id: "my app".into(),
                environment_slug: "feat-auth".into(),
                service_id: "web".into(),
                profile: None,
            }
        );
    }
}
