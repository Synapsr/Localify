use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::infrastructure::file_security::write_private_file;

const FILE_NAME: &str = "public-url-passwords.json";

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
    path: PathBuf,
}

impl PasswordStore {
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(FILE_NAME),
        }
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

    /// Replaces the file; with no password left, deletes it.
    pub fn save(&self, passwords: &BTreeMap<String, StoredPassword>) -> Result<()> {
        if passwords.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => {
                    Err(error).with_context(|| format!("Could not delete {}", self.path.display()))
                }
            };
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        let contents = serde_json::to_vec_pretty(passwords)?;
        write_private_file(&self.path, &contents)
            .with_context(|| format!("Could not write {}", self.path.display()))
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
    fn debug_output_never_shows_a_password() {
        let output = format!("{:?}", passwords());
        assert!(!output.contains("open sesame"), "{output}");
    }
}
