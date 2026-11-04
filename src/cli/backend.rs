use std::fs::{self, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result, bail};
use localify::desktop::DesktopApi;
use localify::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

use crate::branding::BRANDING;

/// How a privileged CLI mutation is executed.
///
/// Direct mode keeps the historical behavior (the process writes the config
/// and talks to the daemon itself, which requires root for the default
/// `/etc` setup). Helper mode forwards the mutation to the root system helper
/// installed by the desktop app, so no sudo is needed.
pub enum WriteBackend {
    Direct,
    Helper(SystemHelperClient),
}

impl WriteBackend {
    /// Picks the backend for a privileged mutation: direct when the config is
    /// writable by this process, otherwise the running system helper.
    ///
    /// Returns an actionable error when neither is available instead of
    /// failing later with a bare permission error.
    pub fn resolve(config_path: &Path) -> Result<Self> {
        // The dev profile has no helper: a helper answering there is left over
        // from an older install.
        let socket_path = current_uid()
            .ok()
            .filter(|_| !BRANDING.is_dev_profile())
            .map(helper_socket_path);
        Self::resolve_with(config_path, socket_path.as_deref())
    }

    fn resolve_with(config_path: &Path, helper_socket: Option<&Path>) -> Result<Self> {
        if config_writable(config_path) {
            return Ok(Self::Direct);
        }

        if let Some(socket) = helper_socket {
            let client = SystemHelperClient::new(socket.to_path_buf());
            if client.ping().is_ok() {
                return Ok(Self::Helper(client));
            }
        }

        bail!(
            "This command needs to modify {}, which requires administrator privileges.\n\
             Re-run it with sudo, or open the {} desktop app once so its system helper \
             can apply changes without sudo.",
            config_path.display(),
            BRANDING.app_name
        )
    }
}

/// Builds a `DesktopApi` that performs mutations in this process, for CLI
/// commands whose write logic lives in the shared desktop API.
pub fn direct_api(config_path: &Path) -> Result<DesktopApi> {
    let binary_path = std::env::current_exe().with_context(|| {
        format!(
            "Failed to resolve the {} executable path",
            BRANDING.cli_command()
        )
    })?;
    Ok(DesktopApi::new_direct(
        config_path.to_path_buf(),
        binary_path,
    ))
}

fn config_writable(config_path: &Path) -> bool {
    if config_path.exists() {
        return OpenOptions::new().write(true).open(config_path).is_ok();
    }

    let Some(parent) = config_path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }

    let probe = parent.join(format!(
        ".{}-write-test-{}",
        BRANDING.slug(),
        std::process::id()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_config_uses_direct_backend() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();

        let backend = WriteBackend::resolve_with(&config_path, None).unwrap();

        assert!(matches!(backend, WriteBackend::Direct));
    }

    #[test]
    fn missing_config_in_writable_directory_uses_direct_backend() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("nested").join("config.toml");

        let backend = WriteBackend::resolve_with(&config_path, None).unwrap();

        assert!(matches!(backend, WriteBackend::Direct));
    }

    #[test]
    fn unwritable_config_without_helper_returns_guidance() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let missing_socket = dir.path().join("helper.sock");
        let error = match WriteBackend::resolve_with(&config_path, Some(&missing_socket)) {
            Ok(_) => panic!("expected an error when config is read-only and no helper runs"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("sudo"));
        assert!(error.to_string().contains("desktop app"));
    }
}
