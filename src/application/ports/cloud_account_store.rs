use super::cloud_api::CloudAccount;

/// Port for the credentials a signed-in machine keeps. The CLI, the app and
/// the daemon all read them (ADR 0020).
pub trait CloudAccountStore {
    /// `None` when this machine is not signed in.
    fn load(&self) -> anyhow::Result<Option<CloudAccount>>;
    fn save(&self, account: &CloudAccount) -> anyhow::Result<()>;
    /// Deleting credentials that are already gone is not an error.
    fn delete(&self) -> anyhow::Result<()>;
}
