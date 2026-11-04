use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

const PROFILE_ENV: &str = "LOCALIFY_PROFILE";
const DEV_PROFILE: &str = "dev";

/// Central product branding.
///
/// Most runtime naming, paths, and suffix behavior should be derived from this
/// constant so product renames stay small and explicit.
#[allow(dead_code)]
pub struct Branding {
    pub app_name: &'static str,
    pub desktop_name: &'static str,
    pub desktop_state_dir_name: &'static str,
    pub dev_desktop_state_dir_name: &'static str,
    pub slug: &'static str,
    pub dev_slug: &'static str,
    pub domain_suffix: &'static str,
    pub dev_domain_suffix: &'static str,
    pub env_prefix: &'static str,
}

pub const BRANDING: Branding = Branding {
    app_name: "Localify",
    desktop_name: "Localify",
    desktop_state_dir_name: "Localify Desktop",
    dev_desktop_state_dir_name: "Localify Dev",
    slug: "localify",
    dev_slug: "localify-dev",
    domain_suffix: ".localify",
    dev_domain_suffix: ".localify.test",
    env_prefix: "LOCALIFY",
};

#[allow(dead_code)]
impl Branding {
    pub fn profile_env(&self) -> &'static str {
        PROFILE_ENV
    }

    pub fn active_profile(&self) -> Option<String> {
        active_profile_from_env_and_args(
            std::env::var(PROFILE_ENV).ok().as_deref(),
            std::env::args_os().next().as_deref(),
        )
    }

    pub fn is_dev_profile(&self) -> bool {
        self.active_profile().as_deref() == Some(DEV_PROFILE)
    }

    pub fn slug(&self) -> &'static str {
        if self.is_dev_profile() {
            self.dev_slug
        } else {
            self.slug
        }
    }

    pub fn domain_suffix(&self) -> &'static str {
        if self.is_dev_profile() {
            self.dev_domain_suffix
        } else {
            self.domain_suffix
        }
    }

    pub fn desktop_state_dir_name(&self) -> &'static str {
        if self.is_dev_profile() {
            self.dev_desktop_state_dir_name
        } else {
            self.desktop_state_dir_name
        }
    }

    pub fn domain_label(&self) -> &'static str {
        self.domain_suffix()
            .strip_prefix('.')
            .unwrap_or(self.domain_suffix())
    }

    /// On macOS, and on Linux with systemd, everything lives in the user's folder and
    /// no Localify process runs as root. Root, or a machine with no systemd such as a
    /// container, keeps the system paths and runs a plain process.
    fn uses_user_directories(&self) -> bool {
        cfg!(target_os = "macos") || self.is_dev_profile() || self.uses_systemd_service()
    }

    /// Whether launchd owns the daemon: production on macOS. The dev profile
    /// uses unprivileged ports, so the app starts its daemon directly.
    pub fn uses_launchd_service(&self) -> bool {
        cfg!(target_os = "macos") && !self.is_dev_profile()
    }

    /// Whether systemd owns the daemon: a normal user on Linux with systemd.
    pub fn uses_systemd_service(&self) -> bool {
        cfg!(target_os = "linux")
            && !self.is_dev_profile()
            && !crate::infrastructure::process::is_root()
            && self.has_systemd()
    }

    /// Whether a service manager holds the ports for a daemon running as the user.
    pub fn uses_system_service(&self) -> bool {
        self.uses_launchd_service() || self.uses_systemd_service()
    }

    /// Whether system changes go through a script of system tools, with one approval.
    pub fn installs_with_one_approval(&self) -> bool {
        cfg!(target_os = "macos") || self.uses_systemd_service()
    }

    /// The check systemd documents for whether it runs this machine.
    pub fn has_systemd(&self) -> bool {
        Path::new("/run/systemd/system").is_dir()
    }

    pub fn config_dir(&self) -> PathBuf {
        if self.uses_user_directories() {
            return self.local_application_support_dir();
        }

        PathBuf::from(format!("/etc/{}", self.slug()))
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    pub fn data_dir(&self) -> PathBuf {
        if self.uses_user_directories() {
            return self.config_dir().join("data");
        }

        self.config_dir()
    }

    pub fn pid_file(&self) -> PathBuf {
        if self.uses_user_directories() {
            return self.config_dir().join(format!("{}.pid", self.slug()));
        }

        PathBuf::from(format!("/var/run/{}.pid", self.slug()))
    }

    pub fn log_file(&self) -> PathBuf {
        if self.uses_user_directories() {
            return self
                .config_dir()
                .join("logs")
                .join(format!("{}.log", self.slug()));
        }

        PathBuf::from(format!("/var/log/{0}/{0}.log", self.slug()))
    }

    pub fn certs_dir(&self) -> PathBuf {
        if self.uses_user_directories() {
            return self.data_dir().join("certs");
        }

        self.config_dir().join("certs")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.config_dir().join(format!("{}.sock", self.slug()))
    }

    pub fn resolver_file(&self) -> PathBuf {
        PathBuf::from("/etc/resolver").join(self.domain_label())
    }

    pub fn resolved_drop_in_file(&self) -> PathBuf {
        PathBuf::from("/etc/systemd/resolved.conf.d").join(format!("{}.conf", self.slug()))
    }

    /// launchd label of the daemon service.
    pub fn service_label(&self) -> String {
        format!("dev.{}.daemon", self.slug())
    }

    /// Label prefix of the root helper older builds installed, one per uid.
    pub fn legacy_helper_prefix(&self) -> String {
        format!("dev.{}.helper", self.slug())
    }

    pub fn ca_name(&self) -> String {
        format!(
            "{} Local Development CA",
            self.local_runtime_name(self.active_profile().as_deref())
        )
    }

    pub fn local_org_name(&self) -> String {
        format!(
            "{} Local Development",
            self.local_runtime_name(self.active_profile().as_deref())
        )
    }

    pub fn cli_command(&self) -> &'static str {
        self.slug()
    }

    /// A command as the user should type it when it needs root. On macOS
    /// Localify asks for the approval itself and refuses to run as root.
    pub fn admin_command(&self, args: &str) -> String {
        if self.installs_with_one_approval() {
            format!("{} {args}", self.cli_command())
        } else {
            format!("sudo {} {args}", self.cli_command())
        }
    }

    pub fn runtime_desktop_name(&self) -> String {
        if cfg!(debug_assertions) {
            format!("{} (Dev)", self.desktop_name)
        } else {
            self.desktop_name.to_string()
        }
    }

    pub fn env_var(&self, suffix: &str) -> String {
        format!("{}_{}", self.env_prefix, suffix)
    }

    fn local_runtime_name(&self, active_profile: Option<&str>) -> &'static str {
        if active_profile == Some(DEV_PROFILE) {
            self.dev_desktop_state_dir_name
        } else {
            self.app_name
        }
    }

    fn local_application_support_dir(&self) -> PathBuf {
        user_home_dir()
            .map(|home| {
                if cfg!(target_os = "macos") {
                    home.join("Library/Application Support")
                        .join(self.desktop_state_dir_name())
                } else {
                    home.join(".config").join(self.slug())
                }
            })
            .unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/{}-{}", self.slug(), std::process::id()))
            })
    }
}

fn active_profile_from_env_and_args(
    profile_env: Option<&str>,
    argv0: Option<&OsStr>,
) -> Option<String> {
    let explicit_profile = profile_env
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if explicit_profile.is_some() {
        return explicit_profile;
    }

    if argv0_command_name(argv0?) == Some(BRANDING.dev_slug) {
        return Some(DEV_PROFILE.to_string());
    }

    None
}

fn argv0_command_name(argv0: &OsStr) -> Option<&str> {
    Path::new(argv0).file_stem()?.to_str()
}

pub fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_profile_env_wins_over_invocation_name() {
        assert_eq!(
            active_profile_from_env_and_args(
                Some("prod"),
                Some(OsStr::new("/usr/local/bin/localify-dev")),
            ),
            Some("prod".into())
        );
    }

    #[test]
    fn localify_dev_invocation_uses_dev_profile() {
        assert_eq!(
            active_profile_from_env_and_args(None, Some(OsStr::new("/usr/local/bin/localify-dev")),),
            Some("dev".into())
        );
        assert_eq!(
            active_profile_from_env_and_args(None, Some(OsStr::new("localify-dev.exe"))),
            Some("dev".into())
        );
    }

    #[test]
    fn localify_invocation_keeps_default_profile() {
        assert_eq!(
            active_profile_from_env_and_args(None, Some(OsStr::new("/usr/local/bin/localify"))),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keeps_every_path_in_the_users_folder() {
        for path in [
            BRANDING.config_path(),
            BRANDING.data_dir(),
            BRANDING.pid_file(),
            BRANDING.log_file(),
            BRANDING.certs_dir(),
            BRANDING.socket_path(),
        ] {
            assert!(!path.starts_with("/etc"), "{}", path.display());
            assert!(!path.starts_with("/var"), "{}", path.display());
            assert!(
                path.to_string_lossy()
                    .contains("Library/Application Support"),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn dev_profile_uses_distinct_local_certificate_identity() {
        assert_eq!(BRANDING.local_runtime_name(None), "Localify");
        assert_eq!(BRANDING.local_runtime_name(Some("dev")), "Localify Dev");
    }
}
