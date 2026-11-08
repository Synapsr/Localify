use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::infrastructure::file_security::write_private_file;
use crate::infrastructure::remote::store::RemoteStore;

pub const FILE_NAME: &str = "cloud.json";
/// Never deleted, so every process locks the same file.
const LOCK_FILE: &str = "cloud.lock";

/// The sign-in lock: `login`, sign-out, revocation and every write under
/// `remote/` take it, so a sign-in is replaced or removed in one step, and a
/// check of who is signed in holds until the write that depends on it is
/// done. Released when the file is dropped.
pub fn sign_in_lock(data_dir: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(data_dir)
        .with_context(|| format!("Could not create {}", data_dir.display()))?;
    let path = data_dir.join(LOCK_FILE);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("Could not open {}", path.display()))?;
    lock.lock()
        .with_context(|| format!("Could not lock {}", path.display()))?;
    Ok(lock)
}

/// What a signed-in machine keeps. The CLI, the app and the daemon all read
/// it; the daemon runs outside the login session, so the Keychain is not an
/// option (ADR 0020).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudCredentials {
    pub api_base: String,
    pub token: String,
    pub login: String,
    pub machine_id: String,
    pub machine_name: String,
}

/// Leaves the token out, so credentials can appear in logs and errors.
impl fmt::Debug for CloudCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudCredentials")
            .field("api_base", &self.api_base)
            .field("token", &"<redacted>")
            .field("login", &self.login)
            .field("machine_id", &self.machine_id)
            .field("machine_name", &self.machine_name)
            .finish()
    }
}

/// `cloud.json` in the data directory, readable by its owner only.
#[derive(Debug, Clone)]
pub struct CredentialsStore {
    path: PathBuf,
}

impl CredentialsStore {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(FILE_NAME),
        }
    }

    /// The folder `cloud.json` lives in.
    pub fn data_dir(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }

    /// `None` when this machine is not signed in.
    pub fn load(&self) -> Result<Option<CloudCredentials>> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not read {}", self.path.display()));
            }
        };
        let credentials = serde_json::from_str(&contents).with_context(|| {
            format!(
                "{} is not valid. Delete it and sign in again.",
                self.path.display()
            )
        })?;
        Ok(Some(credentials))
    }

    /// Replaces the sign-in, under the sign-in lock.
    pub fn save(&self, credentials: &CloudCredentials) -> Result<()> {
        let _lock = sign_in_lock(&self.data_dir())?;
        let contents = serde_json::to_vec_pretty(credentials)?;
        write_private_file(&self.path, &contents)
            .with_context(|| format!("Could not write {}", self.path.display()))
    }

    /// Signs this machine out here: the credentials go, and with them the
    /// remote key, the pins and the remote settings, so a new sign-in starts
    /// over (`docs/remote-machines.md`, "Keys"). Deleting credentials that
    /// are already gone is not an error.
    pub fn delete(&self) -> Result<()> {
        let _lock = sign_in_lock(&self.data_dir())?;
        self.delete_locked()
    }

    /// Signs out only while `cloud.json` still holds `token`, compared and
    /// deleted under the sign-in lock: a revocation of an old token never
    /// takes a newer sign-in with it. `true` when it signed out.
    pub fn delete_if_token(&self, token: &str) -> Result<bool> {
        let _lock = sign_in_lock(&self.data_dir())?;
        match self.load()? {
            Some(credentials) if credentials.token == token => {
                self.delete_locked()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn delete_locked(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not delete {}", self.path.display()));
            }
        }
        RemoteStore::in_data_dir(&self.data_dir()).remove_locked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> CloudCredentials {
        CloudCredentials {
            api_base: "https://api.localify.dev".into(),
            token: "lfy_secret".into(),
            login: "teo-goulois".into(),
            machine_id: "m1".into(),
            machine_name: "laptop".into(),
        }
    }

    #[test]
    fn a_machine_that_never_signed_in_has_no_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn saved_credentials_load_back_and_only_the_owner_can_read_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(&dir.path().join("data"));
        store.save(&credentials()).unwrap();
        assert_eq!(store.load().unwrap(), Some(credentials()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&store.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn deleting_twice_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        store.save(&credentials()).unwrap();
        store.delete().unwrap();
        store.delete().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn signing_out_deletes_the_remote_key_pins_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        store.save(&credentials()).unwrap();
        RemoteStore::in_data_dir(dir.path())
            .identity_for("m1")
            .unwrap();
        store.delete().unwrap();
        assert!(!dir.path().join("remote").exists());
    }

    /// A revocation of token A that waits for the lock while a new login
    /// writes token B: once it runs, it finds B and leaves it.
    #[test]
    fn a_revocation_racing_a_new_login_leaves_the_new_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        store.save(&credentials()).unwrap();

        let login = sign_in_lock(dir.path()).unwrap();
        let revocation = {
            let store = store.clone();
            std::thread::spawn(move || store.delete_if_token("lfy_secret").unwrap())
        };
        // The new login, inside the lock the revocation waits for.
        let newer = CloudCredentials {
            token: "lfy_newer".into(),
            machine_id: "m2".into(),
            ..credentials()
        };
        write_private_file(&store.path, &serde_json::to_vec(&newer).unwrap()).unwrap();
        drop(login);

        assert!(
            !revocation.join().unwrap(),
            "the revocation signed nothing out"
        );
        assert_eq!(store.load().unwrap(), Some(newer));
        // Without a race, the same revocation does sign out.
        store.save(&credentials()).unwrap();
        assert!(store.delete_if_token("lfy_secret").unwrap());
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn debug_output_never_shows_the_token() {
        let output = format!("{:?}", credentials());
        assert!(!output.contains("lfy_secret"));
    }

    #[test]
    fn a_corrupt_file_says_how_to_recover() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        fs::write(&store.path, "not json").unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("sign in again"), "{error}");
    }
}
