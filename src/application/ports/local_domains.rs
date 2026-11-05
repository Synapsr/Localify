use crate::domain::DomainName;

/// Who a local domain belongs to, which decides whether it may get a public
/// URL (`docs/public-urls.md`, "Exposable local domain"): only a project's
/// domains and its namespace are served (ADR 0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalDomainOwner {
    /// A registered domain of this project, by the project's name.
    Project(String),
    /// This project's namespace, `<project-id>.localify`, which nothing
    /// registers, as in a monorepo with `web.journal.localify` only.
    ProjectNamespace(String),
    /// Registered, but in no project.
    NoProject,
    /// Registered by Docker or another provider, outside the config.
    External,
    NotRegistered,
}

/// Port for the local domains of this machine.
pub trait LocalDomains {
    fn owner(&self, domain: &DomainName) -> anyhow::Result<LocalDomainOwner>;
    /// The labels of the domains under `domain` that belong to its project:
    /// `web` and `web--feat` for `web.journal.localify` and
    /// `web--feat.journal.localify`.
    fn labels_under(&self, domain: &DomainName) -> anyhow::Result<Vec<String>>;
}
