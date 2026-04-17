mod commands;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use commands::AppState;
use roxy::branding::BRANDING;
use roxy::desktop::DesktopApi;
use roxy::desktop::dto::StartDaemonRequest;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime, WindowEvent};

const TRAY_ID: &str = "localify-tray";
const TRAY_OPEN: &str = "tray-open";
const TRAY_START: &str = "tray-start";
const TRAY_STOP: &str = "tray-stop";
const TRAY_RESTART: &str = "tray-restart";
const TRAY_QUIT: &str = "tray-quit";

#[derive(Default)]
struct TrayState {
    quitting: AtomicBool,
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

fn sibling_binary_path() -> Option<PathBuf> {
    let binary_name = if cfg!(target_os = "windows") {
        "localify.exe"
    } else {
        BRANDING.cli_command()
    };

    let candidate = std::env::current_exe().ok()?.parent()?.join(binary_name);
    candidate.exists().then_some(candidate)
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

fn daemon_running<R: Runtime>(app: &AppHandle<R>) -> bool {
    let state = app.state::<AppState>();
    state
        .api
        .daemon_status()
        .map(|status| status.running)
        .unwrap_or(false)
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

fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = build_tray_menu(app)?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(BRANDING.app_name)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            if let Err(error) = run_tray_action(app, event.id().as_ref()) {
                eprintln!("tray action '{}' failed: {error}", event.id().as_ref());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }

    #[cfg(target_os = "macos")]
    {
        tray = tray.icon_as_template(true);
    }

    tray.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let api = DesktopApi::new(default_config_path(), default_binary_path());

    tauri::Builder::default()
        .manage(AppState { api })
        .manage(TrayState::default())
        .setup(|app| Ok(setup_tray(app.handle())?))
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
            commands::get_config,
            commands::update_config,
            commands::get_daemon_status,
            commands::list_domains,
            commands::list_projects,
            commands::get_available_port,
            commands::install_roxy,
            commands::setup_cli_access,
            commands::list_port_conflicts,
            commands::kill_port_conflict,
            commands::get_uninstall_preview,
            commands::uninstall_roxy,
            commands::create_project,
            commands::create_project_with_domains,
            commands::update_project,
            commands::remove_project,
            commands::assign_domain_project,
            commands::register_domain,
            commands::unregister_domain,
            commands::add_route,
            commands::remove_route,
            commands::list_routes,
            commands::start_daemon,
            commands::stop_daemon,
            commands::restart_daemon,
            commands::reload_daemon,
            commands::read_logs
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
