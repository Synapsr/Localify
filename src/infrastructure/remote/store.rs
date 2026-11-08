//! `<data dir>/remote/`: this machine's key, its pins and its target
//! settings (`docs/remote-machines.md`, "Keys"). Every file is 0600 and
//! written through a temporary file renamed over it, under a lock on the
//! folder, so the daemon, the app and the CLI never undo each other's
//! change. Signing out deletes the folder.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::noise::{StaticKeys, decode_key, encode_key};
use super::wire::TargetSettings;
use crate::infrastructure::cloud::credentials::FILE_NAME as CREDENTIALS_FILE;
use crate::infrastructure::file_security::write_private_file;
use crate::infrastructure::launch_rev::random_bytes;

const DIR_NAME: &str = "remote";
const IDENTITY_FILE: &str = "identity.json";
const PEERS_FILE: &str = "peers.json";
const SETTINGS_FILE: &str = "settings.json";

/// This machine's remote key, for the machine id it was made for. The
/// encoded private key is wiped from memory when it is dropped.
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub machine_id: String,
    /// base64url without padding.
    pub public_key: String,
    pub private_key: String,
    /// RFC 3339.
    pub created_at: String,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("machine_id", &self.machine_id)
            .field("public_key", &self.public_key)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

impl Identity {
    /// The key pair. An error names the file, never what it holds.
    pub fn keys(&self) -> Result<StaticKeys> {
        let invalid = |half: &str| {
            anyhow!(
                "The {half} key in {DIR_NAME}/{IDENTITY_FILE} is invalid. Sign out and sign in again, then pair this machine again."
            )
        };
        let private = decode_key(&self.private_key).map_err(|_| invalid("private"))?;
        let public = decode_key(&self.public_key).map_err(|_| invalid("public"))?;
        Ok(StaticKeys {
            private: Zeroizing::new(private),
            public,
        })
    }
}

/// A peer's pinned key, by its machine id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub public_key: String,
    /// A label only: names repeat, machine ids do not.
    pub name: String,
    /// RFC 3339.
    pub paired_at: String,
}

pub type Pins = BTreeMap<String, Pin>;

/// A change to the target settings. `None` keeps a switch as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChange {
    pub share: Option<bool>,
    pub control: Option<bool>,
    pub background_starts: Option<bool>,
}

/// Why a settings change was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    #[error("Remote controls need sharing. Turn it on too: --share on --control on")]
    ControlNeedsShare,
    #[error(
        "Background starts need remote controls. Turn them on too: --control on --background-starts on"
    )]
    BackgroundNeedsControl,
}

/// Applies a change: turning `share` off turns the others off, and a switch
/// is never on without the one it needs.
pub fn apply_change(
    current: TargetSettings,
    change: SettingsChange,
) -> Result<TargetSettings, SettingsError> {
    let mut next = TargetSettings {
        share: change.share.unwrap_or(current.share),
        control: change.control.unwrap_or(current.control),
        background_starts: change
            .background_starts
            .unwrap_or(current.background_starts),
    };
    if change.share == Some(false) {
        next.control = false;
        next.background_starts = false;
    }
    if change.control == Some(false) {
        next.background_starts = false;
    }
    if next.control && !next.share {
        return Err(SettingsError::ControlNeedsShare);
    }
    if next.background_starts && !next.control {
        return Err(SettingsError::BackgroundNeedsControl);
    }
    Ok(next)
}

/// Every switch starts off.
pub const DEFAULT_SETTINGS: TargetSettings = TargetSettings {
    share: false,
    control: false,
    background_starts: false,
};

/// Writes refuse once this machine is signed out: a pairing that ends
/// after `logout` never brings the folder back.
#[derive(Debug, Clone)]
pub struct RemoteStore {
    dir: PathBuf,
    credentials: PathBuf,
}

impl RemoteStore {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(DIR_NAME),
            credentials: data_dir.join(CREDENTIALS_FILE),
        }
    }

    /// The key for this sign-in, made on first need. An identity made for
    /// another machine id belongs to an earlier sign-in: the whole folder
    /// goes with it, pins and settings included.
    pub fn identity_for(&self, machine_id: &str) -> Result<Identity> {
        let _lock = self.lock()?;
        if let Some(identity) = self.load_identity()? {
            if identity.machine_id == machine_id {
                return Ok(identity);
            }
            self.clear_contents()?;
        }
        let keys = StaticKeys::generate()?;
        let identity = Identity {
            machine_id: machine_id.to_string(),
            public_key: encode_key(&keys.public),
            private_key: encode_key(&keys.private),
            created_at: now_rfc3339(),
        };
        drop(keys);
        self.write(IDENTITY_FILE, &identity)?;
        Ok(identity)
    }

    /// `None` before the first relay connection.
    pub fn load_identity(&self) -> Result<Option<Identity>> {
        self.read(IDENTITY_FILE)
    }

    /// Empty when nothing is paired.
    pub fn pins(&self) -> Result<Pins> {
        Ok(self.read(PEERS_FILE)?.unwrap_or_default())
    }

    /// Changes the pins under the folder's lock and returns them as saved.
    pub fn update_pins(&self, change: impl FnOnce(&mut Pins)) -> Result<Pins> {
        let _lock = self.lock()?;
        let mut pins = self.pins()?;
        change(&mut pins);
        self.write(PEERS_FILE, &pins)?;
        Ok(pins)
    }

    /// Every switch off when the file does not exist.
    pub fn settings(&self) -> Result<TargetSettings> {
        let settings = self.read(SETTINGS_FILE)?.unwrap_or(DEFAULT_SETTINGS);
        // A file edited by hand still never holds a switch without the one
        // it needs.
        Ok(TargetSettings {
            share: settings.share,
            control: settings.control && settings.share,
            background_starts: settings.background_starts && settings.control && settings.share,
        })
    }

    pub fn update_settings(&self, change: SettingsChange) -> Result<TargetSettings> {
        let _lock = self.lock()?;
        let next = apply_change(self.settings()?, change)?;
        self.write(SETTINGS_FILE, &next)?;
        Ok(next)
    }

    /// Signing out: a new sign-in starts with a new key, no pins and every
    /// switch off. It waits for a write in progress, which then finds the
    /// machine signed out. Deleting what is already gone is not an error.
    pub fn delete_all(&self) -> Result<()> {
        let _lock = match File::open(&self.dir) {
            Ok(lock) => {
                lock.lock()
                    .with_context(|| format!("Could not lock {}", self.dir.display()))?;
                Some(lock)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not open {}", self.dir.display()));
            }
        };
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("Could not delete {}", self.dir.display()))
            }
        }
    }

    /// Empties the folder but keeps it, so the lock held on it stays valid.
    fn clear_contents(&self) -> Result<()> {
        for name in [IDENTITY_FILE, PEERS_FILE, SETTINGS_FILE] {
            let path = self.dir.join(name);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Could not delete {}", path.display()));
                }
            }
        }
        Ok(())
    }

    /// The folder's lock, for a write. Checked again under the lock: a
    /// signed-out machine writes nothing.
    fn lock(&self) -> Result<File> {
        self.require_signed_in()?;
        create_private_dir(&self.dir)?;
        let lock = File::open(&self.dir)
            .with_context(|| format!("Could not open {}", self.dir.display()))?;
        lock.lock()
            .with_context(|| format!("Could not lock {}", self.dir.display()))?;
        self.require_signed_in()?;
        Ok(lock)
    }

    fn require_signed_in(&self) -> Result<()> {
        if self.credentials.exists() {
            return Ok(());
        }
        bail!("This machine is not signed in to Localify Cloud, so it keeps no remote state.")
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.dir.join(name);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => Zeroizing::new(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("Could not read {}", path.display()));
            }
        };
        // Without the parser's message: it can quote what the file holds.
        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|_| anyhow!("{} is not valid", path.display()))
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.dir.join(name);
        let mut suffix = [0_u8; 6];
        random_bytes(&mut suffix)?;
        let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
        let temporary = self.dir.join(format!("{name}.{suffix}.tmp"));
        let contents = Zeroizing::new(serde_json::to_vec_pretty(value)?);
        let written = write_private_file(&temporary, &contents)
            .with_context(|| format!("Could not write {}", temporary.display()))
            .and_then(|()| {
                fs::rename(&temporary, &path)
                    .with_context(|| format!("Could not write {}", path.display()))
            });
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        written
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("Could not create {}", dir.display()))
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|now| {
            now.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A data dir with a `cloud.json`, as a signed-in machine has.
    fn signed_in() -> (tempfile::TempDir, RemoteStore) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CREDENTIALS_FILE), "{}").unwrap();
        let store = RemoteStore::in_data_dir(dir.path());
        (dir, store)
    }

    #[test]
    fn a_signed_out_machine_writes_no_remote_state() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        fs::remove_file(dir.path().join(CREDENTIALS_FILE)).unwrap();
        store.delete_all().unwrap();

        // A pairing that ends after the sign-out pins nothing.
        let refused = store.update_pins(|pins| {
            pins.insert("m2".into(), pin("k"));
        });
        assert!(refused.unwrap_err().to_string().contains("not signed in"));
        assert!(store.update_settings(SettingsChange::default()).is_err());
        assert!(store.identity_for("m1").is_err());
        assert!(!dir.path().join("remote").exists());
    }

    #[test]
    fn an_invalid_key_is_reported_without_what_the_file_holds() {
        let (dir, store) = signed_in();
        let identity = store.identity_for("m1").unwrap();
        let secret = "c2VjcmV0LXByaXZhdGUta2V5LXRoYXQtaXMtYnJva2Vu";
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("remote/identity.json")).unwrap())
                .unwrap();
        saved["privateKey"] = serde_json::json!(secret);
        fs::write(dir.path().join("remote/identity.json"), saved.to_string()).unwrap();

        let error = format!(
            "{:#}",
            store.identity_for("m1").unwrap().keys().unwrap_err()
        );
        assert!(
            error.contains("private key in remote/identity.json is invalid"),
            "{error}"
        );
        for window in secret.as_bytes().windows(6) {
            assert!(
                !error.contains(std::str::from_utf8(window).unwrap()),
                "{error}"
            );
        }
        drop(identity);

        // A file that does not parse is not quoted either.
        fs::write(
            dir.path().join("remote/identity.json"),
            format!("{{\"privateKey\": {secret}}}"),
        )
        .unwrap();
        let error = format!("{:#}", store.load_identity().unwrap_err());
        assert!(!error.contains(&secret[..10]), "{error}");
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_identity_is_made_once_per_machine_id_and_only_the_owner_reads_it() {
        let (dir, store) = signed_in();
        assert_eq!(store.load_identity().unwrap(), None);

        let first = store.identity_for("m1").unwrap();
        assert_eq!(store.identity_for("m1").unwrap(), first);
        assert_eq!(mode(&dir.path().join("remote/identity.json")), 0o600);
        assert_eq!(mode(&dir.path().join("remote")), 0o700);
        first.keys().unwrap();
        assert!(!format!("{first:?}").contains(&first.private_key));
    }

    #[test]
    fn an_identity_of_an_earlier_sign_in_takes_its_pins_and_settings_with_it() {
        let (_dir, store) = signed_in();
        let old = store.identity_for("m1").unwrap();
        store
            .update_pins(|pins| {
                pins.insert("m2".into(), pin("k"));
            })
            .unwrap();
        store
            .update_settings(SettingsChange {
                share: Some(true),
                ..SettingsChange::default()
            })
            .unwrap();

        let new = store.identity_for("m9").unwrap();

        assert_ne!(new.public_key, old.public_key);
        assert!(store.pins().unwrap().is_empty());
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
    }

    fn pin(key: &str) -> Pin {
        Pin {
            public_key: key.into(),
            name: "teo-mini".into(),
            paired_at: "2026-11-08T10:12:00Z".into(),
        }
    }

    #[test]
    fn pins_are_kept_by_machine_id_in_a_private_file() {
        let (dir, store) = signed_in();
        assert!(store.pins().unwrap().is_empty());
        store
            .update_pins(|pins| {
                pins.insert("m2".into(), pin("k2"));
            })
            .unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("remote/peers.json")).unwrap())
                .unwrap();
        assert_eq!(saved["m2"]["publicKey"], "k2");
        assert_eq!(saved["m2"]["pairedAt"], "2026-11-08T10:12:00Z");
        assert_eq!(mode(&dir.path().join("remote/peers.json")), 0o600);
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("remote"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn settings_start_off_and_keep_their_dependencies() {
        let (dir, store) = signed_in();
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);

        let on = SettingsChange {
            share: Some(true),
            control: Some(true),
            background_starts: Some(true),
        };
        let all = store.update_settings(on).unwrap();
        assert!(all.share && all.control && all.background_starts);
        let saved = fs::read_to_string(dir.path().join("remote/settings.json")).unwrap();
        assert!(saved.contains("\"backgroundStarts\": true"), "{saved}");
        assert_eq!(mode(&dir.path().join("remote/settings.json")), 0o600);

        let off = store
            .update_settings(SettingsChange {
                share: Some(false),
                ..SettingsChange::default()
            })
            .unwrap();
        assert_eq!(off, DEFAULT_SETTINGS);

        let refused = store.update_settings(SettingsChange {
            control: Some(true),
            ..SettingsChange::default()
        });
        assert!(refused.unwrap_err().to_string().contains("--share on"));
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
    }

    #[test]
    fn a_hand_edited_file_never_holds_a_switch_without_its_dependency() {
        let (dir, store) = signed_in();
        fs::create_dir_all(dir.path().join("remote")).unwrap();
        fs::write(
            dir.path().join("remote/settings.json"),
            r#"{"share": false, "control": true, "backgroundStarts": true}"#,
        )
        .unwrap();
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
    }

    #[test]
    fn deleting_everything_twice_is_fine() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        store.delete_all().unwrap();
        store.delete_all().unwrap();
        assert!(!dir.path().join("remote").exists());
    }
}
