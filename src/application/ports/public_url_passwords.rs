use std::collections::BTreeMap;
use std::fmt;

/// A password this machine set on a public URL, for the version of the URL's
/// protection it belongs to.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedPassword {
    pub version: u32,
    pub password: String,
}

/// Leaves the password out, so it never reaches a log.
impl fmt::Debug for SavedPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedPassword")
            .field("version", &self.version)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Saved passwords by public URL id.
pub type SavedPasswords = BTreeMap<String, SavedPassword>;

/// Port for the passwords this machine set on its public URLs, kept so it
/// can offer a link that carries one (`docs/public-urls.md`, "Saved
/// passwords"). Localify Cloud keeps only their hash.
pub trait PublicUrlPasswords {
    /// Empty when none were saved.
    fn load(&self) -> anyhow::Result<SavedPasswords>;
    /// Changes them under a lock every process of this machine takes, so the
    /// app and the CLI never undo each other's change, and returns them as
    /// saved. Leaving none removes them.
    fn update(&self, change: &mut dyn FnMut(&mut SavedPasswords))
    -> anyhow::Result<SavedPasswords>;
}
