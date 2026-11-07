//! `localify public-url`: share a project domain at `https://<slug>.localify.fyi`
//! (`docs/public-urls.md`). Every URL asks for a password unless opened.

use anyhow::{Result, bail};
use dialoguer::Confirm;
use serde::Serialize;

use super::cloud::print_refresh;
use super::output::{can_prompt, note, print_json, wants_json};
use crate::application::cloud_errors::ABUSE_CONTACT;
use crate::application::ports::{ProtectionChoice, ProtectionMode, PublicUrlState};
use crate::application::public_urls::{CreatePublicUrl, PublicUrlAndPassword, PublicUrlView};
use crate::branding::BRANDING;
use crate::infrastructure::cloud_adapters::CloudAdapters;

#[derive(Serialize)]
struct Removed<'a> {
    removed: &'a PublicUrlView,
}

pub struct CreateArgs {
    pub local_domain: String,
    pub wildcard: bool,
    pub slug: Option<String>,
    pub password: Option<String>,
    pub public: bool,
}

pub fn list(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let urls = cloud.public_urls().list()?;
    if wants_json(json_flag) {
        return print_json(&urls);
    }
    if urls.is_empty() {
        println!(
            "No public URLs yet. Create one with: {} public-url create <local-domain>",
            BRANDING.cli_command()
        );
        return Ok(());
    }
    for (index, url) in urls.iter().enumerate() {
        if index > 0 {
            println!();
        }
        print_url(url);
    }
    Ok(())
}

pub fn create(args: CreateArgs, json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let protection = if args.public {
        ProtectionChoice::Public
    } else {
        ProtectionChoice::Password(args.password)
    };
    let changed = cloud.public_urls().create(CreatePublicUrl {
        local_domain: args.local_domain,
        wildcard: args.wildcard,
        slug: args.slug,
        protection,
    })?;
    let created = changed.value;
    if json {
        print_json(&created)?;
    } else {
        let url = &created.public_url.url;
        println!("Public URL for {}:", url.local_domain);
        println!("  {}", url.url);
        if url.wildcard {
            println!(
                "  {}  for <label>.{}",
                environment_pattern(&url.url, &url.slug),
                url.local_domain
            );
        }
        print_access(&created.public_url, created.password.as_deref());
    }
    // JSON carries them in `unservableHosts`.
    if !json {
        for host in &created.unservable_hosts {
            eprintln!("Warning: {host}");
        }
    }
    print_refresh(&changed.refresh, json);
    Ok(())
}

pub fn remove(reference: &str, yes: bool, json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let urls = cloud.public_urls();
    let found = urls.find(reference)?;
    let question = format!(
        "Remove {}? Its slug stays reserved for you for 30 days.",
        found.url.url
    );
    if !confirmed(&question, yes, &format!("remove {}", found.url.slug))? {
        note(json, "Nothing was removed.");
        return Ok(());
    }
    let changed = urls.remove(&found.url.id)?;
    if json {
        print_json(&Removed { removed: &found })?;
    } else {
        println!("Removed {}.", found.url.url);
    }
    print_refresh(&changed.refresh, json);
    Ok(())
}

/// Sets a password, or rotates it: generated unless `password` is given.
pub fn protect(
    reference: &str,
    password: Option<String>,
    json_flag: bool,
    cloud: &CloudAdapters,
) -> Result<()> {
    let urls = cloud.public_urls();
    let found = urls.find(reference)?;
    let changed = urls.set_protection(&found.url.id, ProtectionChoice::Password(password))?;
    print_protection(&changed.value, wants_json(json_flag))?;
    print_refresh(&changed.refresh, wants_json(json_flag));
    Ok(())
}

/// Opens a URL to everyone. A person decides that, so it asks first.
pub fn unprotect(reference: &str, yes: bool, json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let urls = cloud.public_urls();
    let found = urls.find(reference)?;
    let question = format!(
        "Open {} to everyone? Anyone with the link reaches {} without a password.",
        found.url.url, found.url.local_domain
    );
    if !confirmed(&question, yes, &format!("unprotect {}", found.url.slug))? {
        note(
            json,
            "Nothing changed: the URL still asks for its password.",
        );
        return Ok(());
    }
    let changed = urls.set_protection(&found.url.id, ProtectionChoice::Public)?;
    print_protection(&changed.value, json)?;
    print_refresh(&changed.refresh, json);
    Ok(())
}

/// Serves a URL from this machine, such as after a reinstall or to move it
/// off a machine that is gone.
pub fn claim(reference: &str, json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let urls = cloud.public_urls();
    let found = urls.find(reference)?;
    let changed = urls.claim(&found.url.id)?;
    if json {
        print_json(&changed.value)?;
    } else {
        println!(
            "This machine serves {} now, from {}.",
            changed.value.url.url, changed.value.url.local_domain
        );
    }
    print_refresh(&changed.refresh, json);
    Ok(())
}

/// Asks a person at a terminal. Without one, only `--yes` goes ahead.
fn confirmed(question: &str, yes: bool, command: &str) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !can_prompt() {
        bail!(
            "{question} Nothing was changed: confirm with `{} public-url {command} --yes`.",
            BRANDING.cli_command()
        );
    }
    Ok(Confirm::new()
        .with_prompt(question)
        .default(false)
        .interact()?)
}

fn print_protection(changed: &PublicUrlAndPassword, json: bool) -> Result<()> {
    if json {
        return print_json(changed);
    }
    println!("{}", changed.public_url.url.url);
    print_access(&changed.public_url, changed.password.as_deref());
    Ok(())
}

/// `password` is set when Localify Cloud generated it.
fn print_access(url: &PublicUrlView, password: Option<&str>) {
    match (url.url.protection.mode, password) {
        (ProtectionMode::Public, _) => {
            println!("Open to everyone: no password.");
        }
        (ProtectionMode::Password, Some(password)) => {
            println!("Password: {password}");
            match &url.password_link {
                Some(link) => print_password_link(link),
                None => println!(
                    "It is shown once only: copy it now. Get a new one with: {} public-url protect {}",
                    BRANDING.cli_command(),
                    url.url.slug
                ),
            }
        }
        (ProtectionMode::Password, None) => {
            println!("Protected by the password you chose.");
            if let Some(link) = &url.password_link {
                print_password_link(link);
            }
        }
    }
}

fn print_password_link(link: &str) {
    println!("Link with password: {link}");
    println!("Opening it signs a browser in without typing the password.");
}

/// Leaves the link with the password out: a list may be on a shared screen.
/// `--json` carries it.
fn print_url(view: &PublicUrlView) {
    let url = &view.url;
    println!("{} -> {}", url.url, url.local_domain);
    if url.wildcard {
        println!(
            "  {} -> <label>.{}",
            environment_pattern(&url.url, &url.slug),
            url.local_domain
        );
    }
    let access = match url.protection.mode {
        ProtectionMode::Password => "password",
        ProtectionMode::Public => "open to everyone",
    };
    let serving = match (&url.machine, view.served_here) {
        (_, true) => "served by this machine".to_string(),
        (Some(machine), false) => format!(
            "served by {}; serve it from here with: {} public-url claim {}",
            machine.name,
            BRANDING.cli_command(),
            url.slug
        ),
        (None, false) => format!(
            "served by no machine; serve it from here with: {} public-url claim {}",
            BRANDING.cli_command(),
            url.slug
        ),
    };
    println!("  {access}, {serving}");
    match url.state {
        Some(PublicUrlState::Suspended) => println!(
            "  Suspended: the plan does not cover it right now, so it does not serve. See the plan with: {} whoami",
            BRANDING.cli_command()
        ),
        Some(PublicUrlState::TakenDown) => println!(
            "  Taken down: it was removed for abuse and cannot serve again. To appeal, write to {ABUSE_CONTACT}."
        ),
        Some(PublicUrlState::Serving) | None => {}
    }
}

/// `https://<slug>--<label>.localify.fyi` from `https://<slug>.localify.fyi`.
fn environment_pattern(url: &str, slug: &str) -> String {
    url.replacen(&format!("://{slug}."), &format!("://{slug}--<label>."), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_pattern_follows_the_slug() {
        assert_eq!(
            environment_pattern("https://shop-1b2c.localify.fyi", "shop-1b2c"),
            "https://shop-1b2c--<label>.localify.fyi"
        );
    }
}
