mod commands;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use commands::{AppState, DeepLinkPayloadDto, DeepLinkRouteDto};
use roxy::branding::BRANDING;
use roxy::desktop::DesktopApi;
use roxy::desktop::dto::{
    DomainInfoDto, OpenInEditorRequest, ProjectInfoDto, RunLocalDevServerRequest,
    StartDaemonRequest,
};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
#[cfg(target_os = "macos")]
use tauri::{
    async_runtime,
    menu::{AboutMetadata, Submenu},
};
use tauri_plugin_deep_link::DeepLinkExt;
#[cfg(target_os = "macos")]
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

const TRAY_ID: &str = "localify-tray";
const TRAY_OPEN: &str = "tray-open";
const TRAY_START: &str = "tray-start";
const TRAY_STOP: &str = "tray-stop";
const TRAY_RESTART: &str = "tray-restart";
const TRAY_QUIT: &str = "tray-quit";
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

#[derive(Debug)]
enum DeepLinkAction {
    Projects,
    SettingsLicensing,
    Logs {
        domain: String,
    },
    InEditor {
        domain: String,
    },
    RunLocalDevServer {
        domain: String,
        target: Option<String>,
    },
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
    target: Option<String>,
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
        ("open", "/projects") => Ok(DeepLinkAction::Projects),
        ("open", "/settings/licensing") => Ok(DeepLinkAction::SettingsLicensing),
        ("open", "/logs") => Ok(DeepLinkAction::Logs {
            domain: query_param(url, "domain")?,
        }),
        ("action", "/open-in-editor") => Ok(DeepLinkAction::InEditor {
            domain: query_param(url, "domain")?,
        }),
        ("action", "/run-local-dev-server") => Ok(DeepLinkAction::RunLocalDevServer {
            domain: query_param(url, "domain")?,
            target: optional_query_param(url, "target"),
        }),
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

fn optional_query_param(url: &tauri::Url, key: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(candidate, value)| (candidate == key).then_some(value.into_owned()))
        .filter(|value| !value.trim().is_empty())
}

fn execute_deep_link_action<R: Runtime>(
    app: &AppHandle<R>,
    action: DeepLinkAction,
    delivery: DeepLinkDelivery,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    match action {
        DeepLinkAction::Projects => deliver_deep_link_payload(
            app,
            DeepLinkPayloadDto {
                route: DeepLinkRouteDto::Projects,
                project_id: None,
                view: None,
            },
            delivery,
        ),
        DeepLinkAction::SettingsLicensing => deliver_deep_link_payload(
            app,
            DeepLinkPayloadDto {
                route: DeepLinkRouteDto::SettingsLicensing,
                project_id: None,
                view: None,
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
                },
                None => DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
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
                })
                .unwrap_or(DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
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
        DeepLinkAction::RunLocalDevServer { domain, target } => {
            let resolved = resolve_domain_project(&state.api, &domain)?;
            let payload = resolved
                .as_ref()
                .and_then(|resolved| resolved.project_id.as_ref())
                .map(|project_id| DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Logs,
                    project_id: Some(project_id.clone()),
                    view: Some("requests".into()),
                })
                .unwrap_or(DeepLinkPayloadDto {
                    route: DeepLinkRouteDto::Projects,
                    project_id: None,
                    view: None,
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

            let Some(target) = target.or(resolved.target) else {
                return Err(format!(
                    "Localify could not find a proxy target to launch for '{}'.",
                    normalize_domain(&domain)
                ));
            };

            state
                .api
                .run_local_dev_server(RunLocalDevServerRequest { root_path, target })
                .map_err(|error| error.to_string())
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
    let target = matched_domain
        .registration
        .routes
        .iter()
        .find(|route| route.path == "/" && route.target_kind == "proxy")
        .or_else(|| {
            matched_domain
                .registration
                .routes
                .iter()
                .find(|route| route.target_kind == "proxy")
        })
        .map(|route| route.target.clone());

    Ok(Some(ResolvedDomainProject {
        project_id,
        root_path,
        target,
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
    let toggle = MenuItem::with_id(app, toggle_id, toggle_label, true, None::<&str>)?;
    let restart = MenuItem::with_id(app, TRAY_RESTART, "Restart daemon", running, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "Quit Localify", true, None::<&str>)?;

    Menu::with_items(app, &[&open, &toggle, &restart, &separator, &quit])
}

pub(crate) fn sync_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    tray.set_menu(Some(build_tray_menu(app)?))?;
    Ok(())
}

fn run_tray_action<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), String> {
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
            if let Err(error) = run_tray_action(app, event.id().as_ref()) {
                eprintln!("tray action '{}' failed: {error}", event.id().as_ref());
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
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let _ = open_main_window(app);
        }));
    }

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
        .manage(TrayState::default());

    #[cfg(target_os = "macos")]
    let builder = builder.manage(UpdateMenuState::default());

    builder
        .setup(|app| {
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
            commands::check_app_update,
            commands::install_app_update,
            commands::get_config,
            commands::update_config,
            commands::get_licensing_status,
            commands::export_settings,
            commands::preview_settings_import,
            commands::import_settings,
            commands::update_keygen_settings,
            commands::activate_license,
            commands::import_offline_license,
            commands::clear_license,
            commands::refresh_license,
            commands::get_daemon_status,
            commands::list_domains,
            commands::list_projects,
            commands::get_available_port,
            commands::detect_project_init_plan,
            commands::install_roxy,
            commands::setup_cli_access,
            commands::setup_node_trust,
            commands::list_port_conflicts,
            commands::kill_port_conflict,
            commands::get_uninstall_preview,
            commands::uninstall_roxy,
            commands::create_project,
            commands::create_project_with_domains,
            commands::init_project_from_folder,
            commands::update_project,
            commands::remove_project,
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
            commands::run_local_dev_server,
            commands::stop_local_dev_server,
            commands::update_launcher_preferences,
            commands::consume_pending_deep_link
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
