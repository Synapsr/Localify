use std::path::PathBuf;

/// Central product branding.
///
/// Most runtime naming, paths, and suffix behavior should be derived from this
/// constant so product renames stay small and explicit.
#[allow(dead_code)]
pub struct Branding {
    pub app_name: &'static str,
    pub desktop_name: &'static str,
    pub slug: &'static str,
    pub domain_suffix: &'static str,
    pub env_prefix: &'static str,
}

pub const BRANDING: Branding = Branding {
    app_name: "Localify",
    desktop_name: "Localify Desktop",
    slug: "localify",
    domain_suffix: ".localify",
    env_prefix: "LOCALIFY",
};

#[allow(dead_code)]
impl Branding {
    pub fn domain_label(&self) -> &'static str {
        self.domain_suffix
            .strip_prefix('.')
            .unwrap_or(self.domain_suffix)
    }

    pub fn config_dir(&self) -> PathBuf {
        PathBuf::from(format!("/etc/{}", self.slug))
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir().join("config.toml")
    }

    pub fn pid_file(&self) -> PathBuf {
        PathBuf::from(format!("/var/run/{}.pid", self.slug))
    }

    pub fn log_file(&self) -> PathBuf {
        PathBuf::from(format!("/var/log/{0}/{0}.log", self.slug))
    }

    pub fn certs_dir(&self) -> PathBuf {
        self.config_dir().join("certs")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.config_dir().join(format!("{}.sock", self.slug))
    }

    pub fn resolver_file(&self) -> PathBuf {
        PathBuf::from("/etc/resolver").join(self.domain_label())
    }

    pub fn resolved_drop_in_file(&self) -> PathBuf {
        PathBuf::from("/etc/systemd/resolved.conf.d").join(format!("{}.conf", self.slug))
    }

    pub fn helper_socket_path(&self, uid: u32) -> PathBuf {
        PathBuf::from(format!("/var/run/{}-helper-{uid}.sock", self.slug))
    }

    pub fn helper_service_prefix(&self) -> String {
        format!("dev.{}.helper", self.slug)
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

    pub fn env_var(&self, suffix: &str) -> String {
        format!("{}_{}", self.env_prefix, suffix)
    }
}
