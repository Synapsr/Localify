use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::infrastructure::file_security::write_private_file;

const FILE_NAME: &str = "public-url-passwords.json";
/// Written in full, then renamed over the file, so a reader never sees half.
const TEMPORARY_FILE_NAME: &str = "public-url-passwords.json.tmp";

/// One entry of the file.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredPassword {
    pub version: u32,
    pub password: String,
}

/// Leaves the password out, so it never reaches a log.
impl fmt::Debug for StoredPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredPassword")
            .field("version", &self.version)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// `public-url-passwords.json` next to `cloud.json`, readable by its owner
/// only: whoever can read that folder holds the token that can change these
/// passwords anyway.
#[derive(Debug, Clone)]
pub struct PasswordStore {
    dir: PathBuf,
    path: PathBuf,
}

impl PasswordStore {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.to_path_buf(),
            path: data_dir.join(FILE_NAME),
        }
    }

    /// Reads, changes and saves the passwords while holding a lock on the
    /// data directory, which the app, the CLI and the MCP server all take.
    /// A file that is not valid JSON starts over empty.
    pub fn update(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, StoredPassword>),
    ) -> Result<BTreeMap<String, StoredPassword>> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Could not create {}", self.dir.display()))?;
        let lock = File::open(&self.dir)
            .with_context(|| format!("Could not open {}", self.dir.display()))?;
        lock.lock()
            .with_context(|| format!("Could not lock {}", self.dir.display()))?;
        let mut passwords = match self.load() {
            Ok(passwords) => passwords,
            Err(error) if error.is::<serde_json::Error>() => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        change(&mut passwords);
        self.save(&passwords)?;
        Ok(passwords)
    }

    /// Passwords by public URL id; empty when the file does not exist.
    pub fn load(&self) -> Result<BTreeMap<String, StoredPassword>> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not read {}", self.path.display()));
            }
        };
        serde_json::from_str(&contents)
            .with_context(|| format!("{} is not valid", self.path.display()))
    }

    /// Replaces the file; with no password left, deletes it, and any
    /// temporary copy an interrupted write left.
    fn save(&self, passwords: &BTreeMap<String, StoredPassword>) -> Result<()> {
        let temporary = self.dir.join(TEMPORARY_FILE_NAME);
        if passwords.is_empty() {
            remove_if_present(&temporary)?;
            return remove_if_present(&self.path);
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Could not create {}", self.dir.display()))?;
        let contents = serde_json::to_vec_pretty(passwords)?;
        let written = write_private_file(&temporary, &contents)
            .with_context(|| format!("Could not write {}", temporary.display()))
            .and_then(|()| {
                fs::rename(&temporary, &self.path)
                    .with_context(|| format!("Could not write {}", self.path.display()))
            });
        if written.is_err() {
            // It holds passwords too.
            let _ = fs::remove_file(&temporary);
        }
        written
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Could not delete {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passwords() -> BTreeMap<String, StoredPassword> {
        BTreeMap::from([(
            "u1".to_string(),
            StoredPassword {
                version: 2,
                password: "open sesame".into(),
            },
        )])
    }

    #[test]
    fn no_file_means_no_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn saved_passwords_load_back_and_only_the_owner_can_read_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(&dir.path().join("data"));
        store.save(&passwords()).unwrap();
        assert_eq!(store.load().unwrap(), passwords());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&store.path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn saving_none_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        store.save(&passwords()).unwrap();
        store.save(&BTreeMap::new()).unwrap();
        assert!(!store.path.exists());
        store.save(&BTreeMap::new()).unwrap();
    }

    #[test]
    fn an_update_changes_what_is_saved_and_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        store.save(&passwords()).unwrap();

        let updated = store
            .update(|all| {
                let password = StoredPassword {
                    version: 1,
                    password: "another".into(),
                };
                all.insert("u2".into(), password);
            })
            .unwrap();

        assert_eq!(updated.len(), 2);
        assert_eq!(store.load().unwrap(), updated);
        assert!(!dir.path().join(TEMPORARY_FILE_NAME).exists());
    }

    #[test]
    fn clearing_also_deletes_what_an_interrupted_write_left() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        store.save(&passwords()).unwrap();
        fs::write(dir.path().join(TEMPORARY_FILE_NAME), "{}").unwrap();

        store.update(|all| all.clear()).unwrap();

        assert!(!store.path.exists());
        assert!(!dir.path().join(TEMPORARY_FILE_NAME).exists());
    }

    #[test]
    fn an_unreadable_file_starts_over_on_update() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        fs::write(&store.path, "{ not json").unwrap();
        assert!(store.load().is_err());

        let updated = store.update(|all| all.extend(passwords())).unwrap();
        assert_eq!(updated, passwords());
    }

    #[test]
    fn debug_output_never_shows_a_password() {
        let output = format!("{:?}", passwords());
        assert!(!output.contains("open sesame"), "{output}");
    }
}
