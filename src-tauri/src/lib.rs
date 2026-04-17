mod commands;

use std::path::PathBuf;

use roxy::branding::BRANDING;
use commands::AppState;
use roxy::desktop::DesktopApi;

fn default_config_path() -> PathBuf {
    std::env::var_os(BRANDING.env_var("CONFIG_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|| BRANDING.config_path())
}

fn default_binary_path() -> PathBuf {
    if let Some(path) = std::env::var_os(BRANDING.env_var("CLI_PATH")) {
        return PathBuf::from(path);
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let api = DesktopApi::new(default_config_path(), default_binary_path());

    tauri::Builder::default()
        .manage(AppState { api })
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
            commands::get_uninstall_preview,
            commands::uninstall_roxy,
            commands::create_project,
            commands::update_project,
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
