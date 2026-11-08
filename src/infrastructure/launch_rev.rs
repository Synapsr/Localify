//! A service's launch revision, `configRev` (docs/remote-machines.md,
//! "Inventory"): the first 16 hex digits of HMAC-SHA-256, under a secret of
//! this machine, over the canonical JSON of what a launch of the service would
//! use. It changes whenever its config entry or any launch profile of its
//! checkout does, inline `[profiles.<name>.env]` values included. The HMAC
//! keeps a low-entropy value from being guessed from a revision, so the
//! revision is the only thing derived from those values that ever leaves this
//! module.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::infrastructure::config::ConfiguredService;
use crate::infrastructure::launch_profiles::{LaunchProfile, LaunchProfiles, ProfileSource};

/// In the data directory.
pub const KEY_FILE: &str = "launch-rev.key";
const KEY_LENGTH: usize = 32;
/// Hex digits of the HMAC a revision keeps.
const REVISION_LENGTH: usize = 16;
const SHA256_BLOCK: usize = 64;

/// This machine's secret for launch revisions. Its `Debug` leaves the bytes
/// out, so it never reaches a log.
pub struct LaunchRevKey([u8; KEY_LENGTH]);

impl fmt::Debug for LaunchRevKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LaunchRevKey(<redacted>)")
    }
}

impl LaunchRevKey {
    /// `<data dir>/launch-rev.key`, made on first need: 32 random bytes,
    /// readable by this user only. It is written whole to a temporary file,
    /// then linked to its name, which fails when another process made it
    /// first: that key wins and is read back, so two processes never compute
    /// revisions under different keys.
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join(KEY_FILE);
        if let Some(key) = read_key(&path)? {
            return Ok(key);
        }
        fs::create_dir_all(data_dir)
            .with_context(|| format!("Failed to create '{}'", data_dir.display()))?;
        let mut bytes = [0_u8; KEY_LENGTH];
        random_bytes(&mut bytes)?;
        let temporary = temporary_path(data_dir)?;
        let published = write_private(&temporary, &bytes).and_then(|()| {
            fs::hard_link(&temporary, &path).or_else(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => Ok(()),
                _ => Err(error),
            })
        });
        let _ = fs::remove_file(&temporary);
        published.with_context(|| format!("Failed to create '{}'", path.display()))?;
        read_key(&path)?.with_context(|| format!("'{}' vanished after it was made", path.display()))
    }

    /// The service's `configRev`.
    pub fn revision(&self, definition: &LaunchDefinition) -> Result<String> {
        let canonical = serde_json::to_vec(definition)
            .context("Failed to encode a launch definition for its revision")?;
        let mac = hmac_sha256(&self.0, &canonical);
        let mut revision: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
        revision.truncate(REVISION_LENGTH);
        Ok(revision)
    }
}

fn read_key(path: &Path) -> Result<Option<LaunchRevKey>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    let Ok(key) = <[u8; KEY_LENGTH]>::try_from(bytes.as_slice()) else {
        bail!(
            "'{}' is not a launch revision key: it holds {} bytes, not {KEY_LENGTH}. Delete it, and a new one is made; every service's configRev then changes once.",
            path.display(),
            bytes.len()
        );
    };
    Ok(Some(LaunchRevKey(key)))
}

/// A name no other writer, in this process or another, picks at once.
fn temporary_path(dir: &Path) -> Result<PathBuf> {
    let mut nonce = [0_u8; 8];
    random_bytes(&mut nonce)?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(dir.join(format!("{KEY_FILE}.{}.{nonce}.tmp", std::process::id())))
}

/// Creates `path` with mode 0600 from the start, writes `bytes` and syncs.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Fills `bytes` from the kernel's random source.
pub fn random_bytes(bytes: &mut [u8]) -> Result<()> {
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(bytes))
        .context("Failed to read random bytes from /dev/urandom")
}

/// HMAC-SHA-256 (RFC 2104).
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0_u8; SHA256_BLOCK];
    if key.len() > SHA256_BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// What a launch of one service would use, in a fixed field order so its
/// JSON is canonical: the service's entry as the config gives it for its
/// environment, and every effective launch profile of its checkout as
/// [`LaunchProfiles::load`] gives them. The executor builds the same one from
/// the definition it runs.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchDefinition {
    service: ServiceEntry,
    profiles: Profiles,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceEntry {
    project_id: String,
    environment: Option<String>,
    service_id: String,
    command: Option<Vec<String>>,
    root: Option<String>,
    path: Option<String>,
    /// The root route's target: the port the service listens on.
    target: Option<String>,
    domains: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum Profiles {
    /// The checkout has no launch profiles.
    None,
    Loaded(Vec<ProfileEntry>),
    /// `localify.toml` does not load: no launch can start until it does. The
    /// message may quote the file, so it stays inside the HMAC.
    Invalid(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileEntry {
    name: String,
    sources: Vec<SourceEntry>,
    danger: bool,
    max_duration_ms: Option<u128>,
    browser_indicator: bool,
    /// `[profiles.<name>.env]`, in key order.
    env: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum SourceEntry {
    File {
        path: String,
    },
    ProtonPassNote {
        vault: String,
        title: String,
    },
    Infisical {
        env: Option<String>,
        path: Option<String>,
    },
}

impl LaunchDefinition {
    /// `service` as it runs in `environment` (the record's slug), and the
    /// result of `LaunchProfiles::load` on its checkout.
    pub fn new(
        service: &ConfiguredService,
        environment: Option<&str>,
        profiles: &Result<Option<LaunchProfiles>>,
    ) -> Self {
        let profiles = match profiles {
            Ok(None) => Profiles::None,
            Ok(Some(profiles)) => {
                Profiles::Loaded(profiles.profiles().map(profile_entry).collect())
            }
            Err(error) => Profiles::Invalid(format!("{error:#}")),
        };
        Self {
            service: ServiceEntry {
                project_id: service.project_id.clone(),
                environment: environment.map(str::to_string),
                service_id: service.id.clone(),
                command: service.command.clone(),
                root: service.project_root_path.clone(),
                path: service.path.clone(),
                target: service.primary_target.as_ref().map(ToString::to_string),
                domains: service.domains.clone(),
            },
            profiles,
        }
    }
}

fn profile_entry(profile: &LaunchProfile) -> ProfileEntry {
    let mut env = profile.env.clone();
    env.sort();
    ProfileEntry {
        name: profile.name.as_str().to_string(),
        sources: profile.sources.iter().map(source_entry).collect(),
        danger: profile.danger,
        max_duration_ms: profile.max_duration.map(|duration| duration.as_millis()),
        browser_indicator: profile.browser_indicator,
        env,
    }
}

fn source_entry(source: &ProfileSource) -> SourceEntry {
    match source {
        ProfileSource::File { path } => SourceEntry::File { path: path.clone() },
        ProfileSource::ProtonPassNote { vault, title } => SourceEntry::ProtonPassNote {
            vault: vault.clone(),
            title: title.clone(),
        },
        ProfileSource::Infisical { env, path } => SourceEntry::Infisical {
            env: env.clone(),
            path: path.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_key_is_private_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");

        let first = LaunchRevKey::load_or_create(&data).unwrap();
        let again = LaunchRevKey::load_or_create(&data).unwrap();

        let path = data.join(KEY_FILE);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(&path).unwrap().len(), KEY_LENGTH);
        assert_eq!(first.0, again.0);
        assert_eq!(
            fs::read_dir(&data).unwrap().count(),
            1,
            "no temporary file left"
        );
        assert_eq!(format!("{first:?}"), "LaunchRevKey(<redacted>)");
    }

    #[test]
    fn concurrent_first_uses_agree_on_one_key() {
        let dir = tempfile::tempdir().unwrap();
        let keys: Vec<[u8; KEY_LENGTH]> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| LaunchRevKey::load_or_create(dir.path()).unwrap().0))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });

        assert!(keys.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn a_damaged_key_is_an_error_not_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(KEY_FILE), b"short").unwrap();

        let error = LaunchRevKey::load_or_create(dir.path()).unwrap_err();

        assert!(format!("{error:#}").contains("Delete it"));
        assert_eq!(fs::read(dir.path().join(KEY_FILE)).unwrap(), b"short");
    }

    #[test]
    fn a_revision_is_16_hex_digits_that_depend_on_the_key() {
        let definition = LaunchDefinition {
            service: ServiceEntry {
                project_id: "app".into(),
                environment: Some("primary".into()),
                service_id: "web".into(),
                command: Some(vec!["pnpm".into(), "dev".into()]),
                root: Some("/code/app".into()),
                path: None,
                target: Some("127.0.0.1:3000".into()),
                domains: vec!["web.localify".into()],
            },
            profiles: Profiles::None,
        };

        let one = LaunchRevKey([1; KEY_LENGTH]).revision(&definition).unwrap();
        let other = LaunchRevKey([2; KEY_LENGTH]).revision(&definition).unwrap();

        assert_eq!(one.len(), REVISION_LENGTH);
        assert!(one.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(one, other);
    }
}
