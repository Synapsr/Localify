//! `<data dir>/remote/`: this machine's key, its pins and its target
//! settings (`docs/remote-machines.md`, "Keys"). Every file is 0600 and
//! written through a temporary file renamed over it, under the sign-in lock,
//! so the daemon, the app and the CLI never undo each other's change, and
//! nothing outlives the sign-in it belongs to. Signing out deletes the
//! folder.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::noise::{StaticKeys, decode_key, decode_private_key, encode_key};
use super::wire::TargetSettings;
use crate::infrastructure::cloud::credentials::{CredentialsStore, sign_in_lock};
use crate::infrastructure::file_security::write_private_file;
use crate::infrastructure::launch_rev::random_bytes;

const DIR_NAME: &str = "remote";
const IDENTITY_FILE: &str = "identity.json";
const PEERS_FILE: &str = "peers.json";
const SETTINGS_FILE: &str = "settings.json";
/// The sign-in the folder belongs to: `{"machineId"}`.
const OWNER_FILE: &str = "owner.json";

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
        let private = decode_private_key(&self.private_key).map_err(|_| invalid("private"))?;
        let public = decode_key(&self.public_key).map_err(|_| invalid("public"))?;
        Ok(StaticKeys { private, public })
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

/// Every write is bound to one sign-in, by its machine id, and checked
/// under the sign-in lock that `login` and sign-out take too
/// ([`sign_in_lock`]): an actor of an earlier sign-in never writes into a
/// newer one, and nothing is written once the machine is signed out.
/// `owner.json` names the sign-in the folder belongs to; files of any other
/// are cleared before a write and ignored by reads.
#[derive(Debug, Clone)]
pub struct RemoteStore {
    dir: PathBuf,
    data_dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Owner {
    machine_id: String,
}

impl RemoteStore {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(DIR_NAME),
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// The key for this sign-in, made on first need. Files of an earlier
    /// sign-in are cleared first, pins and settings included.
    pub fn identity_for(&self, machine_id: &str) -> Result<Identity> {
        let _lock = self.lock_for(machine_id)?;
        match self.load_identity()? {
            Some(identity) if identity.machine_id == machine_id => return Ok(identity),
            // A key of another sign-in: nothing next to it is this one's.
            Some(_) => self.clear(&[IDENTITY_FILE, PEERS_FILE, SETTINGS_FILE])?,
            // Settings this sign-in chose before its first connection stay.
            None => {}
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

    /// Empty when nothing is paired, or the folder is not this sign-in's.
    pub fn pins(&self) -> Result<Pins> {
        if !self.owned_by_current()? {
            return Ok(Pins::new());
        }
        Ok(self.read(PEERS_FILE)?.unwrap_or_default())
    }

    /// Changes the pins of sign-in `machine_id` and returns them as saved.
    pub fn update_pins(&self, machine_id: &str, change: impl FnOnce(&mut Pins)) -> Result<Pins> {
        let _lock = self.lock_for(machine_id)?;
        let mut pins = self.pins()?;
        change(&mut pins);
        self.write(PEERS_FILE, &pins)?;
        Ok(pins)
    }

    /// Every switch off when the file does not exist, or is not this
    /// sign-in's.
    pub fn settings(&self) -> Result<TargetSettings> {
        if !self.owned_by_current()? {
            return Ok(DEFAULT_SETTINGS);
        }
        let settings = self.read(SETTINGS_FILE)?.unwrap_or(DEFAULT_SETTINGS);
        // A file edited by hand still never holds a switch without the one
        // it needs.
        Ok(TargetSettings {
            share: settings.share,
            control: settings.control && settings.share,
            background_starts: settings.background_starts && settings.control && settings.share,
        })
    }

    pub fn update_settings(
        &self,
        machine_id: &str,
        change: SettingsChange,
    ) -> Result<TargetSettings> {
        let _lock = self.lock_for(machine_id)?;
        let next = apply_change(self.settings()?, change)?;
        self.write(SETTINGS_FILE, &next)?;
        Ok(next)
    }

    /// Deletes the folder; the caller holds the sign-in lock, as
    /// `CredentialsStore::delete` does. What is already gone is no error.
    pub(crate) fn remove_locked(&self) -> Result<()> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("Could not delete {}", self.dir.display()))
            }
        }
    }

    /// The sign-in lock, once `cloud.json` names `machine_id`; the folder
    /// then belongs to that sign-in, and any other's files are cleared.
    fn lock_for(&self, machine_id: &str) -> Result<File> {
        let lock = sign_in_lock(&self.data_dir)?;
        if self.current_machine()?.as_deref() != Some(machine_id) {
            bail!(
                "This machine is no longer signed in as {machine_id}, so its remote state is not written."
            );
        }
        create_private_dir(&self.dir)?;
        let owner: Option<Owner> = self.read(OWNER_FILE)?;
        if owner.as_ref().map(|owner| owner.machine_id.as_str()) != Some(machine_id) {
            self.clear(&[IDENTITY_FILE, PEERS_FILE, SETTINGS_FILE])?;
            self.write(
                OWNER_FILE,
                &Owner {
                    machine_id: machine_id.to_string(),
                },
            )?;
        }
        Ok(lock)
    }

    /// The machine id `cloud.json` holds now; `None` when signed out.
    fn current_machine(&self) -> Result<Option<String>> {
        Ok(CredentialsStore::in_data_dir(&self.data_dir)
            .load()?
            .map(|credentials| credentials.machine_id.clone()))
    }

    fn owned_by_current(&self) -> Result<bool> {
        let owner: Option<Owner> = self.read(OWNER_FILE)?;
        Ok(match (owner, self.current_machine()?) {
            (Some(owner), Some(current)) => owner.machine_id == current,
            _ => false,
        })
    }

    fn clear(&self, names: &[&str]) -> Result<()> {
        for name in names {
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
    use crate::infrastructure::cloud::credentials::CloudCredentials;
    use std::os::unix::fs::PermissionsExt;

    fn sign_in(dir: &Path, machine_id: &str) {
        CredentialsStore::in_data_dir(dir)
            .save(&CloudCredentials {
                api_base: "https://api.localify.dev".into(),
                token: format!("lfy_{machine_id}"),
                login: "teo".into(),
                machine_id: machine_id.into(),
                machine_name: "laptop".into(),
            })
            .unwrap();
    }

    /// A data dir signed in as `m1`.
    fn signed_in() -> (tempfile::TempDir, RemoteStore) {
        let dir = tempfile::tempdir().unwrap();
        sign_in(dir.path(), "m1");
        let store = RemoteStore::in_data_dir(dir.path());
        (dir, store)
    }

    fn share_on() -> SettingsChange {
        SettingsChange {
            share: Some(true),
            ..SettingsChange::default()
        }
    }

    #[test]
    fn a_signed_out_machine_writes_no_remote_state() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        CredentialsStore::in_data_dir(dir.path()).delete().unwrap();
        assert!(!dir.path().join("remote").exists());

        // A pairing that ends after the sign-out pins nothing.
        let refused = store.update_pins("m1", |pins| {
            pins.insert("m2".into(), pin("k"));
        });
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("no longer signed in")
        );
        assert!(store.update_settings("m1", share_on()).is_err());
        assert!(store.identity_for("m1").is_err());
        assert!(!dir.path().join("remote").exists());
    }

    /// An actor of the sign-in before writes nothing into the one after:
    /// logout, then login as a new machine id, then a late pin.
    #[test]
    fn a_write_for_an_earlier_sign_in_never_lands_in_a_newer_one() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        CredentialsStore::in_data_dir(dir.path()).delete().unwrap();
        sign_in(dir.path(), "m2");
        store.update_settings("m2", share_on()).unwrap();

        assert!(
            store
                .update_pins("m1", |pins| {
                    pins.insert("peer".into(), pin("k"));
                })
                .is_err()
        );
        assert!(
            store
                .update_settings("m1", SettingsChange::default())
                .is_err()
        );
        assert!(store.pins().unwrap().is_empty());
        assert!(!dir.path().join("remote/peers.json").exists());
        assert!(store.settings().unwrap().share);
    }

    /// Files left by another sign-in (a crash between the two deletions of
    /// a sign-out) are ignored by reads and cleared by the first write.
    #[test]
    fn files_of_another_sign_in_are_ignored_then_cleared() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        store.update_settings("m1", share_on()).unwrap();
        store
            .update_pins("m1", |pins| {
                pins.insert("peer".into(), pin("k"));
            })
            .unwrap();
        // cloud.json replaced as by a login, remote/ left behind.
        sign_in(dir.path(), "m2");

        assert!(store.pins().unwrap().is_empty());
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
        let new = store.identity_for("m2").unwrap();
        assert_eq!(new.machine_id, "m2");
        assert!(!dir.path().join("remote/peers.json").exists());
        assert!(!dir.path().join("remote/settings.json").exists());
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
        assert_eq!(mode(&dir.path().join("cloud.lock")), 0o600);
        first.keys().unwrap();
        assert!(!format!("{first:?}").contains(&first.private_key));
    }

    /// Switches turned on before the first relay connection belong to the
    /// sign-in, and stay when its key is made.
    #[test]
    fn settings_chosen_before_the_key_is_made_stay() {
        let (_dir, store) = signed_in();
        store.update_settings("m1", share_on()).unwrap();
        store.identity_for("m1").unwrap();
        assert!(store.settings().unwrap().share);
    }

    #[test]
    fn a_new_sign_in_starts_with_a_new_key_no_pins_and_switches_off() {
        let (dir, store) = signed_in();
        let old = store.identity_for("m1").unwrap();
        store
            .update_pins("m1", |pins| {
                pins.insert("m2".into(), pin("k"));
            })
            .unwrap();
        store.update_settings("m1", share_on()).unwrap();

        sign_in(dir.path(), "m9");
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
            .update_pins("m1", |pins| {
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
        let all = store.update_settings("m1", on).unwrap();
        assert!(all.share && all.control && all.background_starts);
        let saved = fs::read_to_string(dir.path().join("remote/settings.json")).unwrap();
        assert!(saved.contains("\"backgroundStarts\": true"), "{saved}");
        assert_eq!(mode(&dir.path().join("remote/settings.json")), 0o600);

        let off = store
            .update_settings(
                "m1",
                SettingsChange {
                    share: Some(false),
                    ..SettingsChange::default()
                },
            )
            .unwrap();
        assert_eq!(off, DEFAULT_SETTINGS);

        let refused = store.update_settings(
            "m1",
            SettingsChange {
                control: Some(true),
                ..SettingsChange::default()
            },
        );
        assert!(refused.unwrap_err().to_string().contains("--share on"));
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
    }

    #[test]
    fn a_hand_edited_file_never_holds_a_switch_without_its_dependency() {
        let (dir, store) = signed_in();
        store
            .update_settings("m1", SettingsChange::default())
            .unwrap();
        fs::write(
            dir.path().join("remote/settings.json"),
            r#"{"share": false, "control": true, "backgroundStarts": true}"#,
        )
        .unwrap();
        assert_eq!(store.settings().unwrap(), DEFAULT_SETTINGS);
    }

    #[test]
    fn signing_out_twice_is_fine() {
        let (dir, store) = signed_in();
        store.identity_for("m1").unwrap();
        let credentials = CredentialsStore::in_data_dir(dir.path());
        credentials.delete().unwrap();
        credentials.delete().unwrap();
        assert!(!dir.path().join("remote").exists());
    }
}
