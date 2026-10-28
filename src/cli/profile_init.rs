//! `localify profile init`: sets up `localify.toml` interactively.
//!
//! Asks a few questions, checks every service against the chosen sources
//! (files on disk, note titles in Proton Pass, folder names in Infisical;
//! never values), previews the file, and writes it only after confirmation.

use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, MultiSelect, Select, theme::ColorfulTheme};

use super::context::AppContext;
use super::services::{ProjectCheckout, current_project_checkout};
use crate::branding::BRANDING;
use crate::domain::launch_profile::LaunchProfileName;
use crate::infrastructure::dotenv;
use crate::infrastructure::launch_profiles::{
    DEFAULT_INFISICAL_PATH, InfisicalCli, LaunchProfile, LaunchProfiles, ProfileSource,
    ProtonPassCli, expand_service_template, infisical_env, infisical_path, infisical_project_file,
    launch_profiles_file_name, render_launch_profiles, validate_launch_profiles,
};

const DEVELOPMENT_FILE: &str = ".env.development";

pub fn run(ctx: &AppContext) -> Result<()> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        bail!(
            "'{} profile init' asks questions, so it needs a terminal. Without one, write {} by hand; '{} dev --help' lists the options.",
            BRANDING.cli_command(),
            launch_profiles_file_name(),
            BRANDING.cli_command()
        );
    }

    let checkout = current_project_checkout(ctx)?;
    print_checkout(&checkout);
    let theme = ColorfulTheme::default();
    let mut profiles = existing_profiles(&checkout, &theme)?;
    // Implicit Infisical profiles apply until the file exists, so writing it
    // is a change even when every answer keeps the defaults off.
    let mut changed = !checkout.root.join(launch_profiles_file_name()).exists()
        && infisical_project_file(&checkout.root).is_some();

    if !profiles.iter().any(|profile| profile.name.is_default())
        && let Some(default) = ask_default_profile(&checkout, &theme)?
    {
        profiles.push(default);
        changed = true;
    }

    while ask_to_add_profile(&profiles, &theme)? {
        let profile = ask_profile(&checkout, &profiles, &theme)?;
        profiles.retain(|existing| existing.name != profile.name);
        profiles.push(profile);
        changed = true;
    }

    if !changed {
        eprintln!("No changes.");
        return Ok(());
    }
    write_profiles(&checkout, &profiles, &theme)
}

fn print_checkout(checkout: &ProjectCheckout) {
    eprintln!(
        "Project {} at {}",
        checkout.project_name,
        checkout.root.display()
    );
    for (id, path) in &checkout.services {
        eprintln!("  {id:<16} {}", display_folder(path));
    }
    eprintln!();
}

fn existing_profiles(
    checkout: &ProjectCheckout,
    theme: &ColorfulTheme,
) -> Result<Vec<LaunchProfile>> {
    match LaunchProfiles::load_file(&checkout.root) {
        Ok(Some(existing)) => {
            let profiles: Vec<LaunchProfile> = existing.profiles().cloned().collect();
            let names: Vec<&str> = profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .collect();
            eprintln!(
                "{} already defines: {}\n",
                launch_profiles_file_name(),
                if names.is_empty() {
                    "no profile".into()
                } else {
                    names.join(", ")
                }
            );
            Ok(profiles)
        }
        Ok(None) => {
            if let Some(file) = infisical_project_file(&checkout.root) {
                eprintln!(
                    "Linked to Infisical by {}: without {}, default, staging and prod already read the Infisical environments dev, staging and prod. Write {} only to change that.\n",
                    display_file_name(&file),
                    launch_profiles_file_name(),
                    launch_profiles_file_name()
                );
            }
            Ok(Vec::new())
        }
        Err(error) => {
            eprintln!("{error:#}\n");
            let start_over = Confirm::with_theme(theme)
                .with_prompt(format!(
                    "{} cannot be read. Start from an empty file?",
                    launch_profiles_file_name()
                ))
                .default(false)
                .interact()?;
            if !start_over {
                bail!(
                    "Nothing written. Fix {} and run this again.",
                    launch_profiles_file_name()
                );
            }
            Ok(Vec::new())
        }
    }
}

fn ask_default_profile(
    checkout: &ProjectCheckout,
    theme: &ColorfulTheme,
) -> Result<Option<LaunchProfile>> {
    let found = services_with_file(checkout, DEVELOPMENT_FILE);
    let linked = infisical_project_file(&checkout.root).is_some();
    let mut items = vec![
        "Nothing extra: services keep loading their own .env files".to_string(),
        format!(
            "Load {DEVELOPMENT_FILE} in every service (found in {} of {})",
            found,
            checkout.services.len()
        ),
    ];
    if linked {
        items.push("Read the Infisical environment dev".to_string());
    }
    let choice = Select::with_theme(theme)
        .with_prompt("Without --profile, services start with")
        .items(&items)
        .default(if linked { 2 } else { 0 })
        .interact()?;

    let source = match choice {
        0 => return Ok(None),
        1 => ProfileSource::File {
            path: DEVELOPMENT_FILE.into(),
        },
        _ => ProfileSource::Infisical {
            env: None,
            path: None,
        },
    };
    Ok(Some(LaunchProfile {
        name: default_profile_name(),
        danger: false,
        max_duration: None,
        browser_indicator: false,
        sources: vec![source],
        env: Vec::new(),
    }))
}

fn default_profile_name() -> LaunchProfileName {
    match LaunchProfileName::new(LaunchProfileName::DEFAULT) {
        Ok(name) => name,
        Err(_) => unreachable!("'default' is a valid launch profile name"),
    }
}

fn ask_to_add_profile(profiles: &[LaunchProfile], theme: &ColorfulTheme) -> Result<bool> {
    let has_named_profile = profiles.iter().any(|profile| !profile.name.is_default());
    let prompt = if has_named_profile {
        "Add or replace another profile?"
    } else {
        "Add a profile, such as prod?"
    };
    Ok(Confirm::with_theme(theme)
        .with_prompt(prompt)
        .default(!has_named_profile)
        .interact()?)
}

fn ask_profile(
    checkout: &ProjectCheckout,
    profiles: &[LaunchProfile],
    theme: &ColorfulTheme,
) -> Result<LaunchProfile> {
    let name = ask_profile_name(profiles, theme)?;
    let mut sources = Vec::new();
    loop {
        if let Some(source) = ask_source(checkout, &name, theme)? {
            sources.push(source);
        }
        if sources.is_empty() {
            continue;
        }
        let another = Confirm::with_theme(theme)
            .with_prompt("Add another source? When two sources define a key, the first one wins")
            .default(false)
            .interact()?;
        if !another {
            break;
        }
    }

    let danger = Confirm::with_theme(theme)
        .with_prompt(format!(
            "Mark '{name}' as dangerous? It gets red indicators in the terminal, browser and desktop app, every launch is logged, and it refuses to start if a source is missing"
        ))
        .default(looks_like_production(name.as_str()))
        .interact()?;

    let env = if danger {
        ask_forced_env(checkout, &name, theme)?
    } else {
        Vec::new()
    };

    Ok(LaunchProfile {
        name,
        danger,
        max_duration: None,
        browser_indicator: danger,
        sources,
        env,
    })
}

/// Offers to force safe values for keys that usually trigger real side
/// effects (emails, SMS, push, invoicing, crons). Key names come from the
/// project's local env files; values are never shown.
fn ask_forced_env(
    checkout: &ProjectCheckout,
    name: &LaunchProfileName,
    theme: &ColorfulTheme,
) -> Result<Vec<(String, String)>> {
    let candidates = side_effect_keys(&local_env_keys(checkout));
    if candidates.is_empty() {
        eprintln!(
            "  Tip: to switch off side effects (emails, crons, invoicing) in '{name}', add a [profiles.{name}.env] table with safe values."
        );
        return Ok(Vec::new());
    }

    let labels: Vec<String> = candidates
        .iter()
        .map(|key| format!("{key} = {}", safe_value(key)))
        .collect();
    let defaults: Vec<bool> = candidates
        .iter()
        .map(|key| checked_by_default(key))
        .collect();
    let chosen = MultiSelect::with_theme(theme)
        .with_prompt(format!(
            "Force safe values in '{name}' so a local run cannot send emails, push or invoices? (space to toggle, enter to confirm)"
        ))
        .items(&labels)
        .defaults(&defaults)
        .interact()?;

    Ok(chosen
        .into_iter()
        .map(|index| (candidates[index].clone(), safe_value(&candidates[index])))
        .collect())
}

/// Variable names defined in the root and service env files.
fn local_env_keys(checkout: &ProjectCheckout) -> Vec<String> {
    let folders = std::iter::once(String::new())
        .chain(checkout.services.iter().map(|(_, path)| path.clone()));
    let mut keys: Vec<String> = folders
        .flat_map(|folder| {
            [".env", ".env.development", ".env.local", ".env.example"]
                .map(|file| checkout.root.join(&folder).join(file))
        })
        .filter_map(|path| fs::read_to_string(path).ok())
        .flat_map(|content| {
            dotenv::parse(&content)
                .entries
                .into_iter()
                .map(|(key, _)| key)
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

const SIDE_EFFECT_MARKERS: &[&str] = &[
    "DRY_RUN",
    "_ENABLED",
    "CRON",
    "SMTP",
    "MAIL",
    "RESEND",
    "BREVO",
    "SENDGRID",
    "POSTMARK",
    "TWILIO",
    "SMS",
    "PUSH",
    "ONESIGNAL",
    "WEBHOOK_URL",
];

fn side_effect_keys(keys: &[String]) -> Vec<String> {
    keys.iter()
        .filter(|key| {
            !["NEXT_PUBLIC_", "VITE_", "PUBLIC_", "EXPO_PUBLIC_"]
                .iter()
                .any(|prefix| key.starts_with(prefix))
        })
        .filter(|key| {
            let key = key.to_ascii_uppercase();
            SIDE_EFFECT_MARKERS
                .iter()
                .any(|marker| key.contains(marker))
        })
        .cloned()
        .collect()
}

/// A value that keeps apps booting (validators often check the format) but
/// makes the side effect fail or no-op.
fn safe_value(key: &str) -> String {
    let key = key.to_ascii_uppercase();
    if key.ends_with("DRY_RUN") {
        "true".into()
    } else if key.ends_with("_ENABLED") || key.contains("CRON") {
        "false".into()
    } else if key.contains("HOST") {
        "smtp.disabled.invalid".into()
    } else if key.contains("RESEND") && (key.contains("TOKEN") || key.contains("KEY")) {
        "re_disabled".into()
    } else if key.contains("URL") {
        "http://disabled.invalid".into()
    } else {
        "disabled".into()
    }
}

/// Switches are safe to flip; credentials are only suggested, since a fake
/// one can break a flow you want to debug.
fn checked_by_default(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.ends_with("DRY_RUN") || key.ends_with("_ENABLED") || key.contains("CRON")
}

fn ask_profile_name(
    profiles: &[LaunchProfile],
    theme: &ColorfulTheme,
) -> Result<LaunchProfileName> {
    let suggested = if profiles
        .iter()
        .any(|profile| profile.name.as_str() == "prod")
    {
        String::new()
    } else {
        "prod".to_string()
    };
    loop {
        let raw: String = Input::with_theme(theme)
            .with_prompt("Profile name (used as --profile <name>)")
            .with_initial_text(suggested.clone())
            .interact_text()?;
        let name = match LaunchProfileName::new(raw.trim()) {
            Ok(name) if name.is_default() => {
                eprintln!("  'default' is set up above; pick another name.");
                continue;
            }
            Ok(name) => name,
            Err(error) => {
                eprintln!("  {error}");
                continue;
            }
        };
        let exists = profiles.iter().any(|profile| profile.name == name);
        if exists
            && !Confirm::with_theme(theme)
                .with_prompt(format!("Replace the existing '{name}' profile?"))
                .default(false)
                .interact()?
        {
            continue;
        }
        return Ok(name);
    }
}

fn ask_source(
    checkout: &ProjectCheckout,
    name: &LaunchProfileName,
    theme: &ColorfulTheme,
) -> Result<Option<ProfileSource>> {
    let items = [
        "Infisical: one folder per service in an environment",
        "Proton Pass notes: one note per service holding a whole .env file",
        "Proton Pass references: a file in each service with pass://… values",
        "Local file in each service (values stay in plain text on disk)",
    ];
    let linked = infisical_project_file(&checkout.root).is_some();
    let choice = Select::with_theme(theme)
        .with_prompt(format!("Where do the variables of '{name}' come from?"))
        .items(items)
        .default(if linked { 0 } else { 1 })
        .interact()?;

    match choice {
        0 => ask_infisical(checkout, name, theme),
        1 => ask_proton_pass_note(checkout, name, theme),
        2 => ask_service_file(
            checkout,
            &format!("{}.refs", env_file_for(name.as_str())),
            theme,
        )
        .map(Some),
        _ => ask_service_file(checkout, &env_file_for(name.as_str()), theme).map(Some),
    }
}

fn ask_proton_pass_note(
    checkout: &ProjectCheckout,
    name: &LaunchProfileName,
    theme: &ColorfulTheme,
) -> Result<Option<ProfileSource>> {
    let store = ProtonPassCli;
    let vaults = match store.vault_names() {
        Ok(vaults) if !vaults.is_empty() => vaults,
        Ok(_) => {
            eprintln!("  Proton Pass has no vault this account can read. Pick another source.");
            return Ok(None);
        }
        Err(error) => {
            eprintln!("  {error:#}\n  Pick another source, or fix Proton Pass and run this again.");
            return Ok(None);
        }
    };
    let default_vault = vaults.iter().position(|vault| vault == "Env").unwrap_or(0);
    let vault = vaults[Select::with_theme(theme)
        .with_prompt("Vault")
        .items(&vaults)
        .default(default_vault)
        .interact()?]
    .clone();

    let titles = store
        .item_titles(&vault)
        .with_context(|| format!("Could not list the notes of vault '{vault}'"))?;
    let file = env_file_for(name.as_str());
    let suggested = infer_note_template(&titles, &checkout.services, &file)
        .unwrap_or_else(|| format!("{}/{{service_path}}/{file}", checkout.project_name));

    loop {
        let title: String = Input::with_theme(theme)
            .with_prompt(
                "Note title ({service_path} becomes each service folder, such as apps/web)",
            )
            .with_initial_text(suggested.clone())
            .interact_text()?;
        let source = ProfileSource::ProtonPassNote {
            vault: vault.clone(),
            title: title.trim().to_string(),
        };
        if let Err(error) = validate_source(name, &source) {
            eprintln!("  {error:#}");
            continue;
        }
        print_checks(checkout.services.iter().map(|(id, path)| {
            let expected = expand_service_template(title.trim(), path);
            let found = titles.iter().any(|existing| existing == &expected);
            (id.as_str(), format!("{vault}: {expected}"), found)
        }));
        return Ok(Some(source));
    }
}

fn ask_infisical(
    checkout: &ProjectCheckout,
    name: &LaunchProfileName,
    theme: &ColorfulTheme,
) -> Result<Option<ProfileSource>> {
    if infisical_project_file(&checkout.root).is_none() {
        eprintln!(
            "  No Infisical project is linked to {}. Run 'infisical init' there, then pick Infisical again.",
            checkout.root.display()
        );
        return Ok(None);
    }

    let default_env = infisical_env(name, None);
    loop {
        let env: String = Input::with_theme(theme)
            .with_prompt("Infisical environment slug")
            .with_initial_text(default_env.clone())
            .interact_text()?;
        let path: String = Input::with_theme(theme)
            .with_prompt(
                "Folder of each service ({service_name} becomes the folder name, such as web; {service_path} the full path, such as apps/web)",
            )
            .with_initial_text(DEFAULT_INFISICAL_PATH)
            .interact_text()?;
        let (env, path) = (env.trim().to_string(), path.trim().to_string());
        let source = ProfileSource::Infisical {
            env: (env != default_env).then(|| env.clone()),
            path: (path != DEFAULT_INFISICAL_PATH).then(|| path.clone()),
        };
        if let Err(error) = validate_source(name, &source) {
            eprintln!("  {error:#}");
            continue;
        }
        check_infisical_folders(checkout, &env, &path);
        return Ok(Some(source));
    }
}

/// Marks each service ✓ or ✗ depending on whether its folder exists in the
/// environment. Lists folder names only, one call per parent folder.
fn check_infisical_folders(checkout: &ProjectCheckout, env: &str, path: &str) {
    let store = InfisicalCli;
    // Listing the root first also checks that the environment exists.
    let mut listings: HashMap<String, Vec<String>> = HashMap::new();
    match store.folder_names(&checkout.root, env, "/") {
        Ok(names) => {
            listings.insert("/".into(), names);
        }
        Err(error) => {
            eprintln!("  Could not check the folders: {error:#}");
            return;
        }
    }
    let mut checks = Vec::new();
    for (id, service_path) in &checkout.services {
        let folder = infisical_path(Some(path), service_path);
        let found = match folder.rsplit_once('/') {
            Some((parent, child)) if !child.is_empty() => {
                let parent = if parent.is_empty() { "/" } else { parent };
                if !listings.contains_key(parent) {
                    match store.folder_names(&checkout.root, env, parent) {
                        Ok(names) => {
                            listings.insert(parent.to_string(), names);
                        }
                        Err(error) => {
                            eprintln!("  Could not check the folders: {error:#}");
                            return;
                        }
                    }
                }
                listings[parent].iter().any(|name| name == child)
            }
            _ => true,
        };
        checks.push((id.as_str(), format!("Infisical {env} {folder}"), found));
    }
    print_checks(checks.into_iter());
}

fn display_file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn ask_service_file(
    checkout: &ProjectCheckout,
    suggested: &str,
    theme: &ColorfulTheme,
) -> Result<ProfileSource> {
    let file: String = Input::with_theme(theme)
        .with_prompt("File name in each service folder")
        .with_initial_text(suggested)
        .interact_text()?;
    let file = file.trim().to_string();
    print_checks(checkout.services.iter().map(|(id, path)| {
        let found = checkout.root.join(path).join(&file).is_file();
        (id.as_str(), format!("{}{file}", folder_prefix(path)), found)
    }));
    if file.ends_with(".refs") {
        eprintln!("  Each line holds a reference, such as DATABASE_URL=pass://Vault/Item/password");
    }
    Ok(ProfileSource::File { path: file })
}

fn print_checks<'a>(checks: impl Iterator<Item = (&'a str, String, bool)>) {
    let mut missing = 0;
    for (service, target, found) in checks {
        if !found {
            missing += 1;
        }
        let mark = if found { "✓" } else { "✗" };
        let note = if found { "" } else { " (missing)" };
        eprintln!("  {mark} {service:<16} {target}{note}");
    }
    if missing > 0 {
        eprintln!(
            "  Services marked ✗ start with a warning, or refuse to start if the profile is dangerous."
        );
    }
}

fn write_profiles(
    checkout: &ProjectCheckout,
    profiles: &[LaunchProfile],
    theme: &ColorfulTheme,
) -> Result<()> {
    let content = render_launch_profiles(profiles);
    validate_launch_profiles(&content)?;
    let path = checkout.root.join(launch_profiles_file_name());

    eprintln!("\n{content}");
    let prompt = if path.exists() {
        format!(
            "Replace {}? Comments in the current file are not kept",
            path.display()
        )
    } else {
        format!("Write {}?", path.display())
    };
    if !Confirm::with_theme(theme)
        .with_prompt(prompt)
        .default(true)
        .interact()?
    {
        eprintln!("Nothing written.");
        return Ok(());
    }

    fs::write(&path, content).with_context(|| format!("Failed to write '{}'", path.display()))?;
    eprintln!(
        "Wrote {}. Commit it: it holds addresses, never values.\nStart a service with: {} dev --profile <name>",
        path.display(),
        BRANDING.cli_command()
    );
    Ok(())
}

fn validate_source(name: &LaunchProfileName, source: &ProfileSource) -> Result<()> {
    validate_launch_profiles(&render_launch_profiles(&[LaunchProfile {
        name: name.clone(),
        danger: false,
        max_duration: None,
        browser_indicator: false,
        sources: vec![source.clone()],
        env: Vec::new(),
    }]))
}

fn services_with_file(checkout: &ProjectCheckout, file: &str) -> usize {
    checkout
        .services
        .iter()
        .filter(|(_, path)| checkout.root.join(path).join(file).is_file())
        .count()
}

/// `.env.production` for production-like names, `.env.<name>` otherwise.
fn env_file_for(profile: &str) -> String {
    if looks_like_production(profile) {
        ".env.production".into()
    } else {
        format!(".env.{profile}")
    }
}

fn looks_like_production(profile: &str) -> bool {
    matches!(profile, "prod" | "production" | "live")
}

/// Finds the title pattern existing notes already follow, such as
/// `Acme/{service_path}/.env.production` for `Acme/apps/web/.env.production`.
fn infer_note_template(
    titles: &[String],
    services: &[(String, String)],
    file: &str,
) -> Option<String> {
    let mut prefixes: HashMap<&str, usize> = HashMap::new();
    for title in titles {
        for (_, path) in services.iter().filter(|(_, path)| !path.is_empty()) {
            let suffix = format!("/{path}/{file}");
            if let Some(prefix) = title.strip_suffix(&suffix) {
                *prefixes.entry(prefix).or_default() += 1;
            }
        }
    }
    prefixes
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(left.0)))
        .map(|(prefix, _)| format!("{prefix}/{{service_path}}/{file}"))
}

fn display_folder(path: &str) -> &str {
    if path.is_empty() { "." } else { path }
}

fn folder_prefix(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> Vec<(String, String)> {
        vec![
            ("acme".into(), "apps/web".into()),
            ("admin".into(), "apps/org-space".into()),
            ("email".into(), "apps/email".into()),
        ]
    }

    #[test]
    fn infers_the_title_pattern_of_existing_notes() {
        let titles = vec![
            "Acme/apps/web/.env.production".to_string(),
            "Acme/apps/org-space/.env.production".to_string(),
            "Acme/apps/web/.env.development".to_string(),
            "Other/apps/web/.env.production".to_string(),
        ];

        assert_eq!(
            infer_note_template(&titles, &services(), ".env.production").as_deref(),
            Some("Acme/{service_path}/.env.production")
        );
        assert_eq!(
            infer_note_template(&titles, &services(), ".env.staging"),
            None
        );
    }

    #[test]
    fn suggests_safe_values_for_side_effect_keys() {
        let keys: Vec<String> = [
            "INFAST_DRY_RUN",
            "INFAST_ENABLED",
            "RESEND_TOKEN",
            "SMTP_HOST",
            "DATABASE_URL",
            "NEXT_PUBLIC_IS_LOGIN_ENABLED",
            "STRIPE_SECRET_KEY",
        ]
        .map(String::from)
        .to_vec();

        let candidates = side_effect_keys(&keys);

        assert_eq!(
            candidates,
            vec![
                "INFAST_DRY_RUN",
                "INFAST_ENABLED",
                "RESEND_TOKEN",
                "SMTP_HOST"
            ]
        );
        assert_eq!(safe_value("INFAST_DRY_RUN"), "true");
        assert_eq!(safe_value("INFAST_ENABLED"), "false");
        assert_eq!(safe_value("RESEND_TOKEN"), "re_disabled");
        assert_eq!(safe_value("SMTP_HOST"), "smtp.disabled.invalid");
        assert!(checked_by_default("INFAST_DRY_RUN"));
        assert!(!checked_by_default("RESEND_TOKEN"));
    }

    #[test]
    fn production_like_profiles_use_the_production_env_file() {
        assert_eq!(env_file_for("prod"), ".env.production");
        assert_eq!(env_file_for("production"), ".env.production");
        assert_eq!(env_file_for("staging"), ".env.staging");
        assert!(looks_like_production("live"));
        assert!(!looks_like_production("preview"));
    }

    #[test]
    fn rejects_unknown_placeholders_before_writing() {
        let name = LaunchProfileName::new("prod").unwrap();
        let bad = ProfileSource::ProtonPassNote {
            vault: "Env".into(),
            title: "{project}/.env".into(),
        };
        let good = ProfileSource::ProtonPassNote {
            vault: "Env".into(),
            title: "Acme/{service_path}/.env.production".into(),
        };

        assert!(validate_source(&name, &bad).is_err());
        assert!(validate_source(&name, &good).is_ok());
    }
}
