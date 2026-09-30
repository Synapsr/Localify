//! Resolves launch profile sources into the variables a service starts with.
//!
//! Sources are read in order and the first one that defines a key wins.
//! Values are only held in memory; reports carry key names and counts.

mod config;
mod proton_pass;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub use config::{
    LaunchProfile, LaunchProfiles, ProfileSource, expand_note_title, launch_profiles_file_name,
    render_launch_profiles, validate_launch_profiles,
};
pub use proton_pass::ProtonPassCli;

use crate::domain::launch_profile::LaunchProfileName;
use crate::infrastructure::dotenv;

const REFERENCE_PREFIX: &str = "pass://";

/// Secret storage that profile sources read from. `ProtonPassCli` in
/// production, an in-memory fake in tests.
pub trait SecretStore: Sync {
    /// Returns the note body, or `None` when the vault or note does not exist.
    fn read_note(&self, vault: &str, title: &str) -> Result<Option<String>>;
    /// Returns the referenced field, or `None` when it does not exist.
    fn read_reference(&self, reference: &str) -> Result<Option<String>>;
}

/// What to load for one service launch.
pub struct LaunchRequest<'a> {
    pub profile: Option<&'a LaunchProfile>,
    /// `--env-file` paths, already absolute. They come before profile
    /// sources and must exist.
    pub env_files: &'a [PathBuf],
    pub service_dir: &'a Path,
    /// Service folder relative to the project root, empty at the root.
    pub service_path: &'a str,
}

#[derive(Debug)]
pub struct ResolvedProfile {
    pub name: Option<LaunchProfileName>,
    pub danger: bool,
    pub max_duration: Option<Duration>,
    pub browser_indicator: bool,
    pub variables: Vec<ProfileVariable>,
    pub sources: Vec<SourceReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileVariable {
    pub key: String,
    pub value: String,
    /// Came from Proton Pass: masked in service output.
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    pub label: String,
    pub status: SourceStatus,
    pub warnings: Vec<String>,
    /// Keys whose `pass://` reference does not exist. Not injected.
    pub missing_references: Vec<String>,
    required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    /// Number of keys this source provided after earlier sources won.
    Loaded {
        keys: usize,
    },
    Missing {
        reason: String,
    },
}

impl SourceReport {
    /// Human-readable problem, if the source did not fully load.
    pub fn problem(&self) -> Option<String> {
        match &self.status {
            SourceStatus::Missing { reason } => Some(format!("{}: {reason}", self.label)),
            SourceStatus::Loaded { .. } if !self.missing_references.is_empty() => Some(format!(
                "{}: reference not found for {}",
                self.label,
                self.missing_references.join(", ")
            )),
            SourceStatus::Loaded { .. } => None,
        }
    }
}

impl ResolvedProfile {
    /// Problems to show as warnings; only non-fatal ones survive `resolve`.
    pub fn problems(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter_map(SourceReport::problem)
            .collect()
    }

    pub fn secret_values(&self) -> Vec<(&str, &str)> {
        self.variables
            .iter()
            .filter(|variable| variable.secret)
            .map(|variable| (variable.key.as_str(), variable.value.as_str()))
            .collect()
    }

    pub fn pairs(&self) -> Vec<(String, String)> {
        self.variables
            .iter()
            .map(|variable| (variable.key.clone(), variable.value.clone()))
            .collect()
    }
}

/// Loads every source of a launch and merges them, first source first.
///
/// Fails when `pass-cli` is unusable, when an `--env-file` is missing, or when
/// any source of a dangerous profile is missing: starting on a mix of
/// development and production values is worse than not starting.
pub fn resolve(request: &LaunchRequest<'_>, store: &dyn SecretStore) -> Result<ResolvedProfile> {
    let mut loaded = Vec::new();
    if let Some(profile) = request.profile.filter(|profile| !profile.env.is_empty()) {
        loaded.push(forced_source(profile));
    }
    for path in request.env_files {
        loaded.push(load_file(
            path,
            format!("--env-file {}", path.display()),
            true,
        )?);
    }
    if let Some(profile) = request.profile {
        for source in &profile.sources {
            loaded.push(load_source(source, request, store)?);
        }
    }

    let (mut variables, mut owners) = merge_first_wins(&loaded);
    let mut sources: Vec<SourceReport> = loaded.into_iter().map(|source| source.report).collect();
    resolve_references(&mut variables, &mut owners, &mut sources, store)?;
    count_keys(&mut sources, &owners);

    let resolved = ResolvedProfile {
        name: request.profile.map(|profile| profile.name.clone()),
        danger: request.profile.is_some_and(|profile| profile.danger),
        max_duration: request.profile.and_then(|profile| profile.max_duration),
        browser_indicator: request
            .profile
            .is_some_and(|profile| profile.browser_indicator),
        variables,
        sources,
    };
    enforce_missing_source_policy(&resolved)?;
    Ok(resolved)
}

struct LoadedSource {
    report: SourceReport,
    entries: Vec<(String, String)>,
    secret: bool,
}

/// `[profiles.<name>.env]` comes first so it wins over every other source.
fn forced_source(profile: &LaunchProfile) -> LoadedSource {
    LoadedSource {
        report: SourceReport {
            label: format!(
                "{} [profiles.{}.env]",
                launch_profiles_file_name(),
                profile.name
            ),
            status: SourceStatus::Loaded { keys: 0 },
            warnings: Vec::new(),
            missing_references: Vec::new(),
            required: false,
        },
        entries: profile.env.clone(),
        secret: false,
    }
}

fn load_source(
    source: &ProfileSource,
    request: &LaunchRequest<'_>,
    store: &dyn SecretStore,
) -> Result<LoadedSource> {
    match source {
        ProfileSource::File { path } => load_file(
            &request.service_dir.join(path),
            format!("file {path}"),
            false,
        ),
        ProfileSource::ProtonPassNote { vault, title } => {
            let title = expand_note_title(title, request.service_path);
            let label = format!("Proton Pass note '{title}' (vault {vault})");
            match store.read_note(vault, &title)? {
                Some(body) => Ok(parsed_source(label, &body, true, false)),
                None => Ok(missing_source(label, "note not found".into(), false)),
            }
        }
    }
}

fn load_file(path: &Path, label: String, required: bool) -> Result<LoadedSource> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(parsed_source(label, &content, false, required)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(missing_source(
            label,
            format!("{} does not exist", path.display()),
            required,
        )),
        Err(error) => {
            Err(error).with_context(|| format!("Failed to read env file '{}'", path.display()))
        }
    }
}

fn parsed_source(label: String, content: &str, secret: bool, required: bool) -> LoadedSource {
    let parsed = dotenv::parse(content);
    LoadedSource {
        report: SourceReport {
            label,
            status: SourceStatus::Loaded { keys: 0 },
            warnings: parsed.warnings.iter().map(ToString::to_string).collect(),
            missing_references: Vec::new(),
            required,
        },
        entries: parsed.entries,
        secret,
    }
}

fn missing_source(label: String, reason: String, required: bool) -> LoadedSource {
    LoadedSource {
        report: SourceReport {
            label,
            status: SourceStatus::Missing { reason },
            warnings: Vec::new(),
            missing_references: Vec::new(),
            required,
        },
        entries: Vec::new(),
        secret: false,
    }
}

/// Merges sources so the first one defining a key wins. Returns the
/// variables and, for each, the index of the source that provided it.
fn merge_first_wins(sources: &[LoadedSource]) -> (Vec<ProfileVariable>, Vec<usize>) {
    let mut variables: Vec<ProfileVariable> = Vec::new();
    let mut owners = Vec::new();

    for (index, source) in sources.iter().enumerate() {
        for (key, value) in &source.entries {
            if variables.iter().any(|variable| &variable.key == key) {
                continue;
            }
            variables.push(ProfileVariable {
                key: key.clone(),
                value: value.clone(),
                secret: source.secret,
            });
            owners.push(index);
        }
    }

    (variables, owners)
}

/// Replaces `pass://` values with their Proton Pass value, one `pass-cli`
/// call per reference, in parallel. Unknown references drop their key.
fn resolve_references(
    variables: &mut Vec<ProfileVariable>,
    owners: &mut Vec<usize>,
    sources: &mut [SourceReport],
    store: &dyn SecretStore,
) -> Result<()> {
    let pending: Vec<usize> = (0..variables.len())
        .filter(|&index| variables[index].value.starts_with(REFERENCE_PREFIX))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }

    let results: Vec<Result<Option<String>>> = thread::scope(|scope| {
        let handles: Vec<_> = pending
            .iter()
            .map(|&index| {
                let reference = variables[index].value.as_str();
                scope.spawn(move || store.read_reference(reference))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Proton Pass lookup panicked")))
            })
            .collect()
    });

    let mut dropped = Vec::new();
    for (&index, result) in pending.iter().zip(results) {
        match result? {
            Some(value) => {
                variables[index].value = value;
                variables[index].secret = true;
            }
            None => {
                sources[owners[index]]
                    .missing_references
                    .push(variables[index].key.clone());
                dropped.push(index);
            }
        }
    }
    for index in dropped.into_iter().rev() {
        variables.remove(index);
        owners.remove(index);
    }
    Ok(())
}

fn count_keys(sources: &mut [SourceReport], owners: &[usize]) {
    for (index, source) in sources.iter_mut().enumerate() {
        if let SourceStatus::Loaded { keys } = &mut source.status {
            *keys = owners.iter().filter(|&&owner| owner == index).count();
        }
    }
}

fn enforce_missing_source_policy(resolved: &ResolvedProfile) -> Result<()> {
    let required_problems: Vec<String> = resolved
        .sources
        .iter()
        .filter(|source| source.required)
        .filter_map(SourceReport::problem)
        .collect();
    if !required_problems.is_empty() {
        bail!(
            "Could not load the requested env file:\n  {}",
            required_problems.join("\n  ")
        );
    }

    let problems = resolved.problems();
    if resolved.danger && !problems.is_empty() {
        let name = resolved
            .name
            .as_ref()
            .map(LaunchProfileName::as_str)
            .unwrap_or_default();
        bail!(
            "Profile '{name}' is marked danger = true, so every source must load. Missing:\n  {}\n\
             The service was not started, to avoid running on a mix of development and production values.",
            problems.join("\n  ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct FakeStore {
        notes: HashMap<(String, String), String>,
        references: HashMap<String, String>,
        broken: bool,
        calls: Mutex<Vec<String>>,
    }

    impl FakeStore {
        fn with_note(mut self, vault: &str, title: &str, body: &str) -> Self {
            self.notes.insert((vault.into(), title.into()), body.into());
            self
        }

        fn with_reference(mut self, reference: &str, value: &str) -> Self {
            self.references.insert(reference.into(), value.into());
            self
        }
    }

    impl SecretStore for FakeStore {
        fn read_note(&self, vault: &str, title: &str) -> Result<Option<String>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("note {vault}/{title}"));
            if self.broken {
                bail!("Proton Pass CLI is not signed in.\nRun: pass-cli login");
            }
            Ok(self.notes.get(&(vault.into(), title.into())).cloned())
        }

        fn read_reference(&self, reference: &str) -> Result<Option<String>> {
            self.calls.lock().unwrap().push(format!("ref {reference}"));
            if self.broken {
                bail!("Proton Pass CLI is not signed in.\nRun: pass-cli login");
            }
            Ok(self.references.get(reference).cloned())
        }
    }

    fn profile(name: &str, danger: bool, sources: Vec<ProfileSource>) -> LaunchProfile {
        LaunchProfile {
            name: LaunchProfileName::new(name).unwrap(),
            danger,
            max_duration: None,
            browser_indicator: danger,
            sources,
            env: Vec::new(),
        }
    }

    fn file(path: &str) -> ProfileSource {
        ProfileSource::File { path: path.into() }
    }

    fn note(title: &str) -> ProfileSource {
        ProfileSource::ProtonPassNote {
            vault: "Env".into(),
            title: title.into(),
        }
    }

    fn value<'a>(resolved: &'a ResolvedProfile, key: &str) -> Option<&'a str> {
        resolved
            .variables
            .iter()
            .find(|variable| variable.key == key)
            .map(|variable| variable.value.as_str())
    }

    struct Monorepo {
        root: tempfile::TempDir,
    }

    impl Monorepo {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir_all(root.path().join("apps/web")).unwrap();
            Self { root }
        }

        fn write(&self, relative: &str, content: &str) {
            fs::write(self.root.path().join(relative), content).unwrap();
        }

        fn web_dir(&self) -> PathBuf {
            self.root.path().join("apps/web")
        }
    }

    fn resolve_web(
        repo: &Monorepo,
        profile: Option<&LaunchProfile>,
        env_files: &[PathBuf],
        store: &FakeStore,
    ) -> Result<ResolvedProfile> {
        let service_dir = repo.web_dir();
        resolve(
            &LaunchRequest {
                profile,
                env_files,
                service_dir: &service_dir,
                service_path: "apps/web",
            },
            store,
        )
    }

    #[test]
    fn file_paths_are_relative_to_the_service_not_the_launch_directory() {
        let repo = Monorepo::new();
        repo.write(".env.development", "WHERE=root\n");
        repo.write("apps/web/.env.development", "WHERE=web\n");
        let default = profile("default", false, vec![file(".env.development")]);

        let resolved = resolve_web(&repo, Some(&default), &[], &FakeStore::default()).unwrap();

        assert_eq!(value(&resolved, "WHERE"), Some("web"));
        assert_eq!(resolved.sources[0].status, SourceStatus::Loaded { keys: 1 });
    }

    #[test]
    fn note_title_uses_the_service_path_from_the_project_root() {
        let repo = Monorepo::new();
        let store = FakeStore::default().with_note(
            "Env",
            "Acme/apps/web/.env.production",
            "DATABASE_URL=\"postgres://prod\"\n",
        );
        let prod = profile(
            "prod",
            true,
            vec![note("Acme/{service_path}/.env.production")],
        );

        let resolved = resolve_web(&repo, Some(&prod), &[], &store).unwrap();

        assert_eq!(value(&resolved, "DATABASE_URL"), Some("postgres://prod"));
        assert!(resolved.variables[0].secret);
        assert!(resolved.danger);
    }

    #[test]
    fn first_source_wins_and_env_files_come_first() {
        let repo = Monorepo::new();
        repo.write("apps/web/.env.one", "A=one\nB=one\n");
        repo.write("apps/web/.env.two", "A=two\nB=two\nC=two\n");
        let override_file = repo.root.path().join("override.env");
        fs::write(&override_file, "A=flag\n").unwrap();
        let staging = profile("staging", false, vec![file(".env.one"), file(".env.two")]);

        let resolved = resolve_web(
            &repo,
            Some(&staging),
            std::slice::from_ref(&override_file),
            &FakeStore::default(),
        )
        .unwrap();

        assert_eq!(value(&resolved, "A"), Some("flag"));
        assert_eq!(value(&resolved, "B"), Some("one"));
        assert_eq!(value(&resolved, "C"), Some("two"));
        let counts: Vec<_> = resolved.sources.iter().map(|s| s.status.clone()).collect();
        assert_eq!(
            counts,
            vec![
                SourceStatus::Loaded { keys: 1 },
                SourceStatus::Loaded { keys: 1 },
                SourceStatus::Loaded { keys: 1 },
            ]
        );
    }

    #[test]
    fn references_in_files_are_resolved_and_marked_secret() {
        let repo = Monorepo::new();
        repo.write(
            "apps/web/.env.refs",
            "API_KEY=pass://Env/stripe/key\nPLAIN=value\n",
        );
        let store = FakeStore::default().with_reference("pass://Env/stripe/key", "sk_live_123");
        let staging = profile("staging", false, vec![file(".env.refs")]);

        let resolved = resolve_web(&repo, Some(&staging), &[], &store).unwrap();

        assert_eq!(value(&resolved, "API_KEY"), Some("sk_live_123"));
        assert_eq!(value(&resolved, "PLAIN"), Some("value"));
        assert_eq!(resolved.secret_values(), vec![("API_KEY", "sk_live_123")]);
    }

    #[test]
    fn forced_env_wins_over_notes_and_env_files() {
        let repo = Monorepo::new();
        let override_file = repo.root.path().join("override.env");
        fs::write(&override_file, "INFAST_DRY_RUN=false\n").unwrap();
        let store = FakeStore::default().with_note(
            "Env",
            "Acme/apps/web/.env.production",
            "INFAST_DRY_RUN=false\nRESEND_TOKEN=re_live_token\nDATABASE_URL=postgres://prod\n",
        );
        let mut prod = profile(
            "prod",
            true,
            vec![note("Acme/{service_path}/.env.production")],
        );
        prod.env = vec![
            ("INFAST_DRY_RUN".into(), "true".into()),
            ("RESEND_TOKEN".into(), "re_disabled".into()),
        ];

        let resolved = resolve_web(
            &repo,
            Some(&prod),
            std::slice::from_ref(&override_file),
            &store,
        )
        .unwrap();

        assert_eq!(value(&resolved, "INFAST_DRY_RUN"), Some("true"));
        assert_eq!(value(&resolved, "RESEND_TOKEN"), Some("re_disabled"));
        assert_eq!(value(&resolved, "DATABASE_URL"), Some("postgres://prod"));
        assert_eq!(resolved.sources[0].status, SourceStatus::Loaded { keys: 2 });
        assert!(resolved.sources[0].label.contains("[profiles.prod.env]"));
    }

    #[test]
    fn missing_sources_warn_in_a_normal_profile() {
        let repo = Monorepo::new();
        repo.write("apps/web/.env.refs", "API_KEY=pass://Env/missing/key\n");
        let staging = profile(
            "staging",
            false,
            vec![
                file(".env.absent"),
                note("Acme/{service_path}/.env"),
                file(".env.refs"),
            ],
        );

        let resolved = resolve_web(&repo, Some(&staging), &[], &FakeStore::default()).unwrap();

        assert!(resolved.variables.is_empty());
        let problems = resolved.problems();
        assert_eq!(problems.len(), 3);
        assert!(problems[0].contains("file .env.absent"));
        assert!(problems[1].contains("Acme/apps/web/.env"));
        assert!(problems[2].contains("reference not found for API_KEY"));
    }

    #[test]
    fn missing_sources_fail_a_dangerous_profile() {
        let repo = Monorepo::new();
        repo.write("apps/web/.env.local-part", "A=1\n");
        let prod = profile(
            "prod",
            true,
            vec![
                file(".env.local-part"),
                note("Acme/{service_path}/.env.production"),
            ],
        );

        let error = resolve_web(&repo, Some(&prod), &[], &FakeStore::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("Profile 'prod' is marked danger = true"));
        assert!(error.contains("Acme/apps/web/.env.production"));
        assert!(error.contains("was not started"));
    }

    #[test]
    fn missing_env_file_flag_always_fails() {
        let repo = Monorepo::new();
        let missing = repo.root.path().join("nope.env");

        let error = resolve_web(&repo, None, &[missing], &FakeStore::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("Could not load the requested env file"));
    }

    #[test]
    fn unusable_proton_pass_fails_even_in_a_normal_profile() {
        let repo = Monorepo::new();
        let store = FakeStore {
            broken: true,
            ..FakeStore::default()
        };
        let staging = profile("staging", false, vec![note("x")]);

        let error = format!(
            "{:#}",
            resolve_web(&repo, Some(&staging), &[], &store).unwrap_err()
        );

        assert!(error.contains("pass-cli login"));
    }

    #[test]
    fn shadowed_references_are_not_fetched() {
        let repo = Monorepo::new();
        repo.write("apps/web/.env.first", "API_KEY=local\n");
        repo.write("apps/web/.env.refs", "API_KEY=pass://Env/stripe/key\n");
        let store = FakeStore::default();
        let staging = profile(
            "staging",
            false,
            vec![file(".env.first"), file(".env.refs")],
        );

        let resolved = resolve_web(&repo, Some(&staging), &[], &store).unwrap();

        assert_eq!(value(&resolved, "API_KEY"), Some("local"));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn reports_never_contain_values() {
        let repo = Monorepo::new();
        repo.write(
            "apps/web/.env.refs",
            "TOKEN=pass://Env/t/v\nBROKEN LINE s3cr3t\n",
        );
        let store = FakeStore::default()
            .with_note(
                "Env",
                "Acme/apps/web/.env.production",
                "DB=postgres://s3cr3t\n",
            )
            .with_reference("pass://Env/t/v", "tok_s3cr3t");
        let prod = profile(
            "prod",
            true,
            vec![
                note("Acme/{service_path}/.env.production"),
                file(".env.refs"),
            ],
        );

        let resolved = resolve_web(&repo, Some(&prod), &[], &store).unwrap();
        let rendered = format!("{:?}", resolved.sources);

        assert!(!rendered.contains("s3cr3t"));
        assert!(rendered.contains("line 2"));
    }
}
