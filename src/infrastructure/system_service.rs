//! The one place that picks launchd or systemd for the system setup.
// Both crates compile this module: the CLI and the desktop API each use it.
#![allow(dead_code)]

use std::path::Path;

use anyhow::Result;

use crate::infrastructure::{linux_system, macos_system};

/// The script root has to run for this request, if anything needs changing. Built after
/// the CA exists, because on Linux the script carries it to the system store.
pub fn pending_script(request: &macos_system::SystemRequest) -> Result<Option<String>> {
    if cfg!(target_os = "linux") {
        let changes = linux_system::changes(request)?;
        return Ok((!changes.is_empty()).then(|| changes.script()));
    }
    let changes = request.changes()?;
    Ok((!changes.is_empty()).then(|| changes.script()))
}

/// The script that takes the setup back out.
pub fn removal_script(cli_link: Option<&Path>) -> Result<String> {
    if cfg!(target_os = "linux") {
        return Ok(linux_system::removal_for_this_machine().script());
    }
    Ok(macos_system::removal_for_this_machine(cli_link)?.script())
}

pub fn service_runs_config(config_path: &Path) -> bool {
    if cfg!(target_os = "linux") {
        return linux_system::service_runs_config(config_path);
    }
    macos_system::service_runs_config(config_path)
}

pub fn system_setup_done() -> bool {
    if cfg!(target_os = "linux") {
        return linux_system::system_setup_done();
    }
    macos_system::system_setup_done()
}
