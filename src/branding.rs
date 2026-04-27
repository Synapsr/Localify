use std::path::PathBuf;

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
        std::env::var(PROFILE_ENV)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
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

    pub fn config_dir(&self) -> PathBuf {
        if self.is_dev_profile() {
            return self.local_application_support_dir();
        }

        PathBuf::from(format!("/etc/{}", self.slug()))
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    pub fn data_dir(&self) -> PathBuf {
        if self.is_dev_profile() {
            return self.config_dir().join("data");
        }

        self.config_dir()
    }

    pub fn pid_file(&self) -> PathBuf {
        if self.is_dev_profile() {
            return self.config_dir().join(format!("{}.pid", self.slug()));
        }

        PathBuf::from(format!("/var/run/{}.pid", self.slug()))
    }

    pub fn log_file(&self) -> PathBuf {
        if self.is_dev_profile() {
            return self
                .config_dir()
                .join("logs")
                .join(format!("{}.log", self.slug()));
        }

        PathBuf::from(format!("/var/log/{0}/{0}.log", self.slug()))
    }

    pub fn certs_dir(&self) -> PathBuf {
        if self.is_dev_profile() {
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

    pub fn helper_socket_path(&self, uid: u32) -> PathBuf {
        PathBuf::from(format!("/var/run/{}-helper-{uid}.sock", self.slug()))
    }

    pub fn helper_service_prefix(&self) -> String {
        format!("dev.{}.helper", self.slug())
    }

    pub fn ca_name(&self) -> String {
        format!("{} Local Development CA", self.app_name)
    }

    pub fn local_org_name(&self) -> String {
        format!("{} Local Development", self.app_name)
    }

    pub fn cli_command(&self) -> &'static str {
        self.slug
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

    fn local_application_support_dir(&self) -> PathBuf {
        user_home_dir()
            .map(|home| {
                home.join("Library/Application Support")
                    .join(self.desktop_state_dir_name())
            })
            .unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/{}-{}", self.slug(), std::process::id()))
            })
    }
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}
