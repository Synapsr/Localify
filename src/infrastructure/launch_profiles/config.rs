//! `localify.toml`: launch profiles declared at the root of a project checkout.
//!
//! The file lives next to the code (and in every worktree) because it only
//! holds addresses — file paths, vault names, note titles — never values.
//! Without it, a project linked to Infisical gets implicit profiles.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::infisical;
use crate::branding::BRANDING;
use crate::domain::launch_profile::LaunchProfileName;

/// Placeholder expanded to the service folder, relative to the project root.
pub const SERVICE_PATH_PLACEHOLDER: &str = "{service_path}";
/// Placeholder expanded to the last segment of the service folder: `web` for
/// `apps/web`. Infisical folders are usually named after the app.
pub const SERVICE_NAME_PLACEHOLDER: &str = "{service_name}";
/// Infisical folder read when a source does not set `path`.
pub const DEFAULT_INFISICAL_PATH: &str = "/{service_name}";
/// Infisical environment read by the `default` profile when a source does
/// not set `env`. Other profiles read the environment of the same name.
pub const DEFAULT_INFISICAL_ENV: &str = "dev";
/// Profiles a project linked to Infisical gets without `localify.toml`:
/// Infisical's default environments, with `prod` dangerous.
const IMPLICIT_PROFILES: &[(&str, bool)] =
    &[("default", false), ("staging", false), ("prod", true)];

/// File name of the project launch profile file.
pub fn launch_profiles_file_name() -> String {
    format!("{}.toml", BRANDING.slug)
}

/// Launch profiles of one project checkout.
#[derive(Debug, Clone)]
pub struct LaunchProfiles {
    /// `localify.toml`, or the Infisical project file for implicit profiles.
    pub path: PathBuf,
    profiles: BTreeMap<LaunchProfileName, LaunchProfile>,
    implicit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchProfile {
    pub name: LaunchProfileName,
    pub danger: bool,
    pub max_duration: Option<Duration>,
    /// Prefix the tab title and show a pill in the app's pages. Defaults to
    /// `danger`.
    pub browser_indicator: bool,
    pub sources: Vec<ProfileSource>,
    /// `[profiles.<name>.env]`: values forced over every source, typically to
    /// switch off side effects (emails, crons, invoicing) when running
    /// locally on production data.
    pub env: Vec<(String, String)>,
}

/// Where profile variables come from. Order matters: the first source that
/// defines a key wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSource {
    /// Dotenv file relative to the service folder. Values starting with
    /// `pass://` are resolved through Proton Pass at launch.
    File { path: String },
    /// Proton Pass note whose body is a complete dotenv file.
    ProtonPassNote { vault: String, title: String },
    /// Every secret of one folder of an Infisical environment, from the
    /// project linked by `.infisical.yaml` at the project root. `None` keeps
    /// the default: see `infisical_env` and `DEFAULT_INFISICAL_PATH`.
    Infisical {
        env: Option<String>,
        path: Option<String>,
    },
}

impl LaunchProfiles {
    /// Reads the profiles of a project root: `localify.toml`, or implicit
    /// Infisical profiles when the project only has `.infisical.yaml`.
    /// Returns `None` when it has neither, so services launch exactly as
    /// before.
    pub fn load(root: &Path) -> Result<Option<Self>> {
        if let Some(profiles) = Self::load_file(root)? {
            return Ok(Some(profiles));
        }
        Ok(infisical::project_file(root).map(implicit_profiles))
    }

    /// Reads `localify.toml` only, for `localify profile init`, which writes
    /// that file.
    pub fn load_file(root: &Path) -> Result<Option<Self>> {
        let path = root.join(launch_profiles_file_name());
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
            }
        };

        let profiles = parse_profiles(&content)
            .with_context(|| format!("Invalid launch profiles in '{}'", path.display()))?;
        Ok(Some(Self {
            path,
            profiles,
            implicit: false,
        }))
    }

    #[allow(dead_code)] // Used by the desktop through the library crate.
    pub fn profiles(&self) -> impl Iterator<Item = &LaunchProfile> {
        self.profiles.values()
    }

    /// Picks the profile for a launch: the requested one, which must exist,
    /// or `default` when nothing is requested and the project defines it.
    pub fn select(&self, requested: Option<&str>) -> Result<Option<&LaunchProfile>> {
        let Some(requested) = requested else {
            return Ok(self
                .profiles
                .values()
                .find(|profile| profile.name.is_default()));
        };

        let found = self
            .profiles
            .values()
            .find(|profile| profile.name.as_str() == requested);
        match found {
            Some(profile) => Ok(Some(profile)),
            None if self.implicit => bail!(
                "Launch profile '{requested}' does not exist. Without {}, {} offers {} for the Infisical project linked by '{}'.\n\
                 Declare other profiles in {}, for example:\n\n\
                 [profiles.{requested}]\n\
                 sources = [{{ infisical = {{ env = \"{requested}\" }} }}]",
                launch_profiles_file_name(),
                BRANDING.app_name,
                self.available_names(),
                self.path.display(),
                launch_profiles_file_name(),
            ),
            None => bail!(
                "Launch profile '{requested}' is not defined in '{}'. Available profiles: {}.",
                self.path.display(),
                self.available_names()
            ),
        }
    }

    fn available_names(&self) -> String {
        if self.profiles.is_empty() {
            return "none".into();
        }
        self.profiles
            .keys()
            .map(LaunchProfileName::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchProfilesFileDto {
    #[serde(default)]
    profiles: BTreeMap<String, LaunchProfileDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchProfileDto {
    #[serde(default)]
    danger: bool,
    #[serde(default)]
    max_duration: Option<String>,
    #[serde(default)]
    browser_indicator: Option<bool>,
    #[serde(default)]
    sources: Vec<ProfileSourceDto>,
    #[serde(default)]
    env: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileSourceDto {
    File(String),
    ProtonPassNote(ProtonPassNoteDto),
    Infisical(InfisicalDto),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InfisicalDto {
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtonPassNoteDto {
    vault: String,
    title: String,
}

fn implicit_profiles(project_file: PathBuf) -> LaunchProfiles {
    let profiles = IMPLICIT_PROFILES
        .iter()
        .filter_map(|&(name, danger)| Some((LaunchProfileName::new(name).ok()?, danger)))
        .map(|(name, danger)| {
            let profile = LaunchProfile {
                name: name.clone(),
                danger,
                max_duration: None,
                browser_indicator: danger,
                sources: vec![ProfileSource::Infisical {
                    env: None,
                    path: None,
                }],
                env: Vec::new(),
            };
            (name, profile)
        })
        .collect();
    LaunchProfiles {
        path: project_file,
        profiles,
        implicit: true,
    }
}

fn parse_profiles(content: &str) -> Result<BTreeMap<LaunchProfileName, LaunchProfile>> {
    let file: LaunchProfilesFileDto = toml::from_str(content)?;
    let mut profiles = BTreeMap::new();

    for (name, dto) in file.profiles {
        let profile = profile_from_dto(&name, dto)?;
        profiles.insert(profile.name.clone(), profile);
    }

    Ok(profiles)
}

fn profile_from_dto(name: &str, dto: LaunchProfileDto) -> Result<LaunchProfile> {
    let name = LaunchProfileName::new(name)?;
    if name.is_default() && dto.danger {
        bail!(
            "Profile 'default' cannot set danger = true: it runs whenever --profile is omitted. Move these sources to a named profile."
        );
    }

    let max_duration = dto
        .max_duration
        .as_deref()
        .map(|value| parse_max_duration(&name, value))
        .transpose()?;
    let sources = dto
        .sources
        .into_iter()
        .map(|source| source_from_dto(&name, source))
        .collect::<Result<Vec<_>>>()?;
    let env = dto
        .env
        .into_iter()
        .map(|(key, value)| forced_variable(&name, key, value))
        .collect::<Result<Vec<_>>>()?;

    Ok(LaunchProfile {
        name,
        danger: dto.danger,
        max_duration,
        browser_indicator: dto.browser_indicator.unwrap_or(dto.danger),
        sources,
        env,
    })
}

/// One `[profiles.<name>.env]` entry. Strings, booleans and numbers are
/// accepted so `INFAST_DRY_RUN = true` reads naturally.
fn forced_variable(
    name: &LaunchProfileName,
    key: String,
    value: toml::Value,
) -> Result<(String, String)> {
    if !is_env_key(&key) {
        bail!(
            "Profile '{name}' sets '{key}' in [profiles.{name}.env]; variable names use letters, digits and '_' and do not start with a digit."
        );
    }
    let value = match value {
        toml::Value::String(value) => value,
        toml::Value::Boolean(value) => value.to_string(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        _ => bail!(
            "Profile '{name}' sets '{key}' in [profiles.{name}.env] to a table or list; use a string, a number or a boolean."
        ),
    };
    Ok((key, value))
}

fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_max_duration(name: &LaunchProfileName, value: &str) -> Result<Duration> {
    let duration = humantime::parse_duration(value.trim()).with_context(|| {
        format!("Profile '{name}' has an invalid max_duration '{value}'. Use a duration such as \"4h\" or \"90m\".")
    })?;
    if duration.is_zero() {
        bail!("Profile '{name}' has max_duration = \"{value}\"; it must be longer than zero.");
    }
    Ok(duration)
}

fn source_from_dto(name: &LaunchProfileName, source: ProfileSourceDto) -> Result<ProfileSource> {
    match source {
        ProfileSourceDto::File(path) => {
            if path.trim().is_empty() {
                bail!("Profile '{name}' has a file source with an empty path.");
            }
            Ok(ProfileSource::File { path })
        }
        ProfileSourceDto::ProtonPassNote(note) => {
            if note.vault.trim().is_empty() || note.title.trim().is_empty() {
                bail!("Profile '{name}' has a proton_pass_note source without a vault or title.");
            }
            validate_template(name, "note title", &note.title)?;
            Ok(ProfileSource::ProtonPassNote {
                vault: note.vault,
                title: note.title,
            })
        }
        ProfileSourceDto::Infisical(source) => {
            if let Some(env) = &source.env
                && !is_environment_slug(env)
            {
                bail!(
                    "Profile '{name}' has an infisical source with env = \"{env}\". Use the environment slug shown in Infisical, such as \"dev\" or \"prod\"."
                );
            }
            if let Some(path) = &source.path {
                if !path.starts_with('/') {
                    bail!(
                        "Profile '{name}' has an infisical source with path = \"{path}\". Infisical folders start with '/', such as \"/{{service_name}}\"."
                    );
                }
                validate_template(name, "Infisical path", path)?;
            }
            Ok(ProfileSource::Infisical {
                env: source.env,
                path: source.path,
            })
        }
    }
}

fn is_environment_slug(env: &str) -> bool {
    !env.is_empty()
        && env
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn validate_template(name: &LaunchProfileName, what: &str, template: &str) -> Result<()> {
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(length) = rest[start..].find('}') else {
            bail!("Profile '{name}' has an unclosed '{{' in {what} '{template}'.");
        };
        let placeholder = &rest[start..start + length + 1];
        if placeholder != SERVICE_PATH_PLACEHOLDER && placeholder != SERVICE_NAME_PLACEHOLDER {
            bail!(
                "Profile '{name}' uses unknown placeholder '{placeholder}' in {what} '{template}'. Only {SERVICE_PATH_PLACEHOLDER} and {SERVICE_NAME_PLACEHOLDER} are supported."
            );
        }
        rest = &rest[start + length + 1..];
    }
    Ok(())
}

/// Writes profiles back as `localify.toml`, `default` first. Only fields that
/// differ from their defaults are written, so the file stays short.
pub fn render_launch_profiles(profiles: &[LaunchProfile]) -> String {
    let mut ordered: Vec<&LaunchProfile> = profiles.iter().collect();
    ordered.sort_by_key(|profile| (!profile.name.is_default(), profile.name.clone()));

    let mut output = String::from(
        "# Launch profiles for `localify dev --profile <name>`.\n\
         # Addresses only (files, vaults, note titles), never values: commit this file.\n",
    );
    for profile in ordered {
        output.push_str(&format!("\n[profiles.{}]\n", profile.name));
        if profile.danger {
            output.push_str("danger = true\n");
        }
        if let Some(duration) = profile.max_duration {
            let duration = humantime::format_duration(duration).to_string();
            output.push_str(&format!("max_duration = {}\n", toml_string(&duration)));
        }
        if profile.browser_indicator != profile.danger {
            output.push_str(&format!(
                "browser_indicator = {}\n",
                profile.browser_indicator
            ));
        }
        output.push_str("sources = [\n");
        for source in &profile.sources {
            output.push_str(&format!("  {},\n", render_source(source)));
        }
        output.push_str("]\n");
        if !profile.env.is_empty() {
            output.push_str(&format!(
                "\n# Forced over the sources, to switch off side effects\n[profiles.{}.env]\n",
                profile.name
            ));
            for (key, value) in &profile.env {
                output.push_str(&format!("{key} = {}\n", toml_string(value)));
            }
        }
    }
    output
}

fn render_source(source: &ProfileSource) -> String {
    match source {
        ProfileSource::File { path } => format!("{{ file = {} }}", toml_string(path)),
        ProfileSource::ProtonPassNote { vault, title } => format!(
            "{{ proton_pass_note = {{ vault = {}, title = {} }} }}",
            toml_string(vault),
            toml_string(title)
        ),
        ProfileSource::Infisical { env, path } => {
            let fields: Vec<String> = [("env", env), ("path", path)]
                .into_iter()
                .filter_map(|(key, value)| {
                    value
                        .as_deref()
                        .map(|value| format!("{key} = {}", toml_string(value)))
                })
                .collect();
            if fields.is_empty() {
                "{ infisical = {} }".into()
            } else {
                format!("{{ infisical = {{ {} }} }}", fields.join(", "))
            }
        }
    }
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

/// Checks rendered content before it is written, with the same rules as
/// `LaunchProfiles::load`.
pub fn validate_launch_profiles(content: &str) -> Result<()> {
    parse_profiles(content).map(|_| ())
}

/// Expands `{service_path}` and `{service_name}` in a note title or an
/// Infisical path. For a service at the project root a placeholder and its
/// slash disappear, so `Acme/{service_path}/.env.production` becomes
/// `Acme/.env.production`.
pub fn expand_service_template(template: &str, service_path: &str) -> String {
    let service_path = service_path.trim_matches('/');
    let service_name = service_path.rsplit('/').next().unwrap_or_default();
    let mut expanded = template.to_string();
    for (placeholder, value) in [
        (SERVICE_PATH_PLACEHOLDER, service_path),
        (SERVICE_NAME_PLACEHOLDER, service_name),
    ] {
        expanded = if value.is_empty() {
            expanded
                .replace(&format!("{placeholder}/"), "")
                .replace(&format!("/{placeholder}"), "")
                .replace(placeholder, "")
        } else {
            expanded.replace(placeholder, value)
        };
    }
    expanded
}

/// Infisical environment a source reads: its own `env`, else `dev` for the
/// `default` profile and the profile's name otherwise.
pub fn infisical_env(profile: &LaunchProfileName, env: Option<&str>) -> String {
    match env {
        Some(env) => env.to_string(),
        None if profile.is_default() => DEFAULT_INFISICAL_ENV.into(),
        None => profile.as_str().to_string(),
    }
}

/// Infisical folder a source reads for one service, always starting with
/// `/`: `/web` for `apps/web` with the default path, `/` at the root.
pub fn infisical_path(path: Option<&str>, service_path: &str) -> String {
    let expanded = expand_service_template(path.unwrap_or(DEFAULT_INFISICAL_PATH), service_path);
    let trimmed = expanded.trim_matches('/');
    format!("/{trimmed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[profiles.default]
sources = [{ file = ".env.development" }]

[profiles.prod]
danger = true
max_duration = "4h"
sources = [
  { proton_pass_note = { vault = "Env", title = "Acme/{service_path}/.env.production" } },
  { file = ".env.prod.refs" },
]
"#;

    fn write_profiles(content: &str) -> (tempfile::TempDir, LaunchProfiles) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(launch_profiles_file_name()), content).unwrap();
        let profiles = LaunchProfiles::load(dir.path()).unwrap().unwrap();
        (dir, profiles)
    }

    fn load_error(content: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(launch_profiles_file_name()), content).unwrap();
        format!("{:#}", LaunchProfiles::load(dir.path()).unwrap_err())
    }

    #[test]
    fn missing_file_means_no_profiles() {
        let dir = tempfile::tempdir().unwrap();

        assert!(LaunchProfiles::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn parses_the_documented_example() {
        let (_dir, profiles) = write_profiles(EXAMPLE);

        let prod = profiles.select(Some("prod")).unwrap().unwrap();
        assert!(prod.danger);
        assert_eq!(prod.max_duration, Some(Duration::from_secs(4 * 3600)));
        assert_eq!(
            prod.sources,
            vec![
                ProfileSource::ProtonPassNote {
                    vault: "Env".into(),
                    title: "Acme/{service_path}/.env.production".into(),
                },
                ProfileSource::File {
                    path: ".env.prod.refs".into()
                },
            ]
        );

        assert!(prod.browser_indicator);

        let default = profiles.select(None).unwrap().unwrap();
        assert_eq!(default.name.as_str(), "default");
        assert!(!default.danger);
        assert!(!default.browser_indicator);
    }

    #[test]
    fn omitted_profile_without_default_selects_nothing() {
        let (_dir, profiles) = write_profiles("[profiles.prod]\ndanger = true\n");

        assert!(profiles.select(None).unwrap().is_none());
    }

    #[test]
    fn unknown_requested_profile_lists_available_ones() {
        let (_dir, profiles) = write_profiles(EXAMPLE);

        let error = profiles.select(Some("staging")).unwrap_err().to_string();
        assert!(error.contains("'staging' is not defined"));
        assert!(error.contains("default, prod"));
    }

    #[test]
    fn browser_indicator_can_be_turned_off_or_on() {
        let (_dir, profiles) = write_profiles(
            "[profiles.prod]\ndanger = true\nbrowser_indicator = false\n\n[profiles.staging]\nbrowser_indicator = true\n",
        );

        assert!(
            !profiles
                .select(Some("prod"))
                .unwrap()
                .unwrap()
                .browser_indicator
        );
        assert!(
            profiles
                .select(Some("staging"))
                .unwrap()
                .unwrap()
                .browser_indicator
        );
    }

    #[test]
    fn forced_env_accepts_scalars_and_rejects_bad_names() {
        let (_dir, profiles) = write_profiles(
            "[profiles.prod]\n[profiles.prod.env]\nINFAST_DRY_RUN = true\nRETRIES = 0\nSMTP_HOST = \"smtp.disabled.invalid\"\n",
        );
        let prod = profiles.select(Some("prod")).unwrap().unwrap();

        assert_eq!(
            prod.env,
            vec![
                ("INFAST_DRY_RUN".to_string(), "true".to_string()),
                ("RETRIES".to_string(), "0".to_string()),
                ("SMTP_HOST".to_string(), "smtp.disabled.invalid".to_string()),
            ]
        );
        assert!(load_error("[profiles.prod.env]\n\"1BAD\" = \"x\"\n").contains("variable names"));
        assert!(load_error("[profiles.prod.env]\nLIST = [1]\n").contains("a table or list"));
    }

    #[test]
    fn default_profile_cannot_be_dangerous() {
        let error = load_error("[profiles.default]\ndanger = true\n");

        assert!(error.contains("'default' cannot set danger = true"));
    }

    #[test]
    fn typos_are_rejected_instead_of_disabling_guardrails() {
        assert!(load_error("[profiles.prod]\ndangr = true\n").contains("unknown field"));
        assert!(load_error("[profile.prod]\ndanger = true\n").contains("unknown field"));
        assert!(load_error("[profiles.prod]\nsources = [{ files = \"x\" }]\n").contains("files"));
    }

    #[test]
    fn rejects_invalid_names_durations_and_placeholders() {
        assert!(load_error("[profiles.Prod]\n").contains("invalid"));
        assert!(load_error("[profiles.prod]\nmax_duration = \"soon\"\n").contains("max_duration"));
        assert!(
            load_error("[profiles.prod]\nmax_duration = \"0s\"\n").contains("longer than zero")
        );
        assert!(
            load_error(
                "[profiles.prod]\nsources = [{ proton_pass_note = { vault = \"Env\", title = \"{project}/.env\" } }]\n"
            )
            .contains("unknown placeholder '{project}'")
        );
    }

    #[test]
    fn rendered_profiles_read_back_identically() {
        let (_dir, original) = write_profiles(
            "[profiles.prod.env]\nINFAST_DRY_RUN = true\nRESEND_TOKEN = \"re_disabled\"\n\n[profiles.prod]\ndanger = true\nmax_duration = \"90m\"\nsources = [{ proton_pass_note = { vault = \"Env\", title = \"A \\\"B\\\"/{service_path}/.env.production\" } }]\n\n[profiles.default]\nsources = [{ file = \".env.development\" }]\n\n[profiles.staging]\nbrowser_indicator = true\nsources = [{ file = \".env.staging.refs\" }, { file = \".env.development\" }]\n\n[profiles.preview]\nsources = [{ infisical = {} }, { infisical = { env = \"preview-1\", path = \"/apps/{service_path}\" } }]\n",
        );
        let profiles: Vec<LaunchProfile> = original.profiles().cloned().collect();

        let rendered = render_launch_profiles(&profiles);
        let (_dir, reloaded) = write_profiles(&rendered);

        assert!(rendered.find("[profiles.default]") < rendered.find("[profiles.prod]"));
        assert_eq!(reloaded.profiles().cloned().collect::<Vec<_>>(), profiles);
        assert!(validate_launch_profiles(&rendered).is_ok());
    }

    #[test]
    fn expands_service_placeholders() {
        let template = "Acme/{service_path}/.env.production";

        assert_eq!(
            expand_service_template(template, "apps/web"),
            "Acme/apps/web/.env.production"
        );
        assert_eq!(
            expand_service_template(template, ""),
            "Acme/.env.production"
        );
        assert_eq!(expand_service_template("{service_path}/.env", ""), ".env");
        assert_eq!(expand_service_template("Acme/{service_path}", ""), "Acme");
        assert_eq!(
            expand_service_template("Acme/{service_name}/.env", "apps/web"),
            "Acme/web/.env"
        );
    }

    #[test]
    fn infisical_sources_parse_with_their_defaults() {
        let (_dir, profiles) = write_profiles(
            "[profiles.default]\nsources = [{ infisical = {} }]\n\n[profiles.prod]\ndanger = true\nsources = [{ infisical = { env = \"production\", path = \"/{service_path}\" } }]\n",
        );

        let default = profiles.select(None).unwrap().unwrap();
        assert_eq!(
            default.sources,
            vec![ProfileSource::Infisical {
                env: None,
                path: None
            }]
        );
        let prod = profiles.select(Some("prod")).unwrap().unwrap();
        assert_eq!(
            prod.sources,
            vec![ProfileSource::Infisical {
                env: Some("production".into()),
                path: Some("/{service_path}".into()),
            }]
        );
        assert!(
            load_error("[profiles.prod]\nsources = [{ infisical = { env = \"Prod env\" } }]\n")
                .contains("environment slug")
        );
        assert!(
            load_error("[profiles.prod]\nsources = [{ infisical = { path = \"web\" } }]\n")
                .contains("start with '/'")
        );
        assert!(
            load_error("[profiles.prod]\nsources = [{ infisical = { project = \"x\" } }]\n")
                .contains("unknown field")
        );
    }

    #[test]
    fn infisical_env_and_path_follow_the_profile_and_the_service() {
        let default = LaunchProfileName::new("default").unwrap();
        let staging = LaunchProfileName::new("staging").unwrap();

        assert_eq!(infisical_env(&default, None), "dev");
        assert_eq!(infisical_env(&staging, None), "staging");
        assert_eq!(infisical_env(&staging, Some("preview")), "preview");
        assert_eq!(infisical_path(None, "apps/web"), "/web");
        assert_eq!(infisical_path(None, ""), "/");
        assert_eq!(
            infisical_path(Some("/{service_path}/"), "apps/web"),
            "/apps/web"
        );
        assert_eq!(infisical_path(Some("/shared"), "apps/web"), "/shared");
    }

    #[test]
    fn a_project_linked_to_infisical_gets_implicit_profiles() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".infisical.yaml"), "version: 1\n").unwrap();

        let profiles = LaunchProfiles::load(dir.path()).unwrap().unwrap();
        let names: Vec<(&str, bool)> = profiles
            .profiles()
            .map(|profile| (profile.name.as_str(), profile.danger))
            .collect();
        assert_eq!(
            names,
            vec![("default", false), ("prod", true), ("staging", false)]
        );
        assert!(
            profiles
                .select(Some("prod"))
                .unwrap()
                .unwrap()
                .browser_indicator
        );
        let error = profiles.select(Some("preview")).unwrap_err().to_string();
        assert!(error.contains(".infisical.yaml"));
        assert!(error.contains("env = \"preview\""));
        assert!(LaunchProfiles::load_file(dir.path()).unwrap().is_none());

        fs::write(
            dir.path().join(launch_profiles_file_name()),
            "[profiles.qa]\n",
        )
        .unwrap();
        let declared = LaunchProfiles::load(dir.path()).unwrap().unwrap();
        assert_eq!(declared.profiles().count(), 1);
    }
}
