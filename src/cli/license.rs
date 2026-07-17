use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use localify::desktop::dto::{ActivateLicenseRequest, ImportOfflineLicenseRequest, LicensingDto};

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::infrastructure::licensing::{
    KeygenClient, ResolvedLicenseStatus, build_offline_imported_license,
    current_machine_fingerprint, mask_fingerprint, mask_license_key, resolve_license_state,
};

pub fn status(ctx: &AppContext) -> Result<()> {
    let config = ctx.config_store.load()?;
    let resolved = resolve_license_state(&config.licensing);
    let limits = &resolved.limits;
    let configured = config.licensing.keygen.is_configured();
    let machine = current_machine_fingerprint().ok();
    let status = match resolved.status {
        ResolvedLicenseStatus::NotConfigured => "not configured",
        ResolvedLicenseStatus::Unlicensed => "unlicensed",
        ResolvedLicenseStatus::Licensed => "licensed",
        ResolvedLicenseStatus::LicensedOffline => "licensed (offline)",
        ResolvedLicenseStatus::Invalid => "invalid",
    };

    println!("License status: {status}");
    println!("Plan: {}", limits.plan);
    println!(
        "Limits: {} projects, {} domains per project",
        format_limit(limits.max_projects),
        format_limit(limits.max_domains_per_project)
    );

    if configured {
        println!(
            "Keygen API: {}",
            config.licensing.keygen.effective_api_url()
        );
        println!("Account: {}", config.licensing.keygen.effective_account());
        if let Some(product) = config.licensing.keygen.effective_product() {
            println!("Product: {product}");
        }
        println!(
            "Public key: {}",
            if config.licensing.keygen.effective_public_key().is_some() {
                "configured"
            } else {
                "not configured"
            }
        );
    } else {
        println!("Keygen is not configured in the current config file.");
    }

    if let Some(machine) = machine {
        println!(
            "Current machine: {} ({})",
            machine
                .name
                .as_deref()
                .or(machine.hostname.as_deref())
                .unwrap_or("Unnamed device"),
            mask_fingerprint(&machine.fingerprint)
        );
    }

    if let Some(license) = config.licensing.license {
        println!("Stored license: {}", mask_license_key(&license.key));
        if let Some(code) = license.validation_code.as_deref() {
            println!("Validation: {code}");
        }
        if let Some(detail) = license.validation_detail.as_deref()
            && !detail.trim().is_empty()
        {
            println!("Details: {detail}");
        }
        if let Some(machine_id) = license.machine_id.as_deref() {
            println!("Activated machine ID: {machine_id}");
        }
        if license.response_signature_verified {
            println!("Signed response: verified");
        }
        if let Some(signature_date) = license.response_signature_date.as_deref() {
            println!("Signature date: {signature_date}");
        }
    } else {
        println!("No license key is stored locally.");
    }

    println!(
        "Offline cache: {}",
        if config.licensing.offline_license.is_some() {
            "present"
        } else {
            "missing"
        }
    );
    if let Some(offline) = resolved.offline_license {
        println!("Offline algorithm: {}", offline.algorithm);
        if let Some(expiry) = offline.file_expiry.as_deref() {
            println!("Offline file expiry: {expiry}");
        }
    }
    if let Some(error) = resolved.last_error.as_deref()
        && !error.trim().is_empty()
    {
        println!("Last error: {error}");
    }

    Ok(())
}

pub fn activate(key: String, ctx: &AppContext) -> Result<()> {
    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let licensing = client.activate_license(ActivateLicenseRequest { key })?;
        print_licensing_summary("License activated", &licensing);
        return Ok(());
    }

    let machine =
        current_machine_fingerprint().context("Failed to identify this machine for activation")?;
    let config = ctx.config_store.load()?;
    let client = KeygenClient::new()?;
    let validation =
        client.validate_and_activate_license_key(&config.licensing.keygen, &key, &machine)?;

    if !validation.valid {
        bail!(validation.detail);
    }

    ctx.config_store
        .store_licensing(Some(validation.license.clone()), validation.offline_license)?;

    println!(
        "License activated: {}",
        validation
            .license
            .plan
            .clone()
            .unwrap_or_else(|| "Licensed".into())
    );
    println!("Key: {}", mask_license_key(&validation.license.key));
    println!("Machine: {}", mask_fingerprint(&machine.fingerprint));
    if let Some(machine_id) = validation.license.machine_id.as_deref() {
        println!("Machine ID: {machine_id}");
    }

    Ok(())
}

pub fn refresh(ctx: &AppContext) -> Result<()> {
    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let licensing = client.refresh_license()?;
        print_licensing_summary("License refreshed", &licensing);
        return Ok(());
    }

    let machine =
        current_machine_fingerprint().context("Failed to identify this machine for refresh")?;
    let config = ctx.config_store.load()?;
    let Some(existing_license) = config.licensing.license else {
        bail!("No stored license key was found. Use 'localify license activate <key>' first.");
    };

    let client = KeygenClient::new()?;
    let validation = client.validate_and_activate_license_key(
        &config.licensing.keygen,
        &existing_license.key,
        &machine,
    )?;
    let mut stored = validation.license;
    if stored.machine_id.is_none()
        && existing_license.machine_fingerprint == stored.machine_fingerprint
    {
        stored.machine_id = existing_license.machine_id.clone();
    }
    if !validation.valid {
        stored.validation_code = Some(validation.code);
        stored.validation_detail = Some(validation.detail.clone());
    }

    let offline_license = validation
        .offline_license
        .or(config.licensing.offline_license.clone());
    ctx.config_store
        .store_licensing(Some(stored.clone()), offline_license)?;

    if !validation.valid {
        bail!(validation.detail);
    }

    println!("License refreshed: {}", mask_license_key(&stored.key));
    println!("Machine: {}", mask_fingerprint(&machine.fingerprint));
    if let Some(machine_id) = stored.machine_id.as_deref() {
        println!("Machine ID: {machine_id}");
    }

    Ok(())
}

pub fn clear(ctx: &AppContext) -> Result<()> {
    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        client.clear_license()?;
        println!("Cleared the stored license from the local config.");
        return Ok(());
    }

    ctx.config_store.store_licensing(None, None)?;
    println!("Cleared the stored license from the local config.");
    Ok(())
}

pub fn import_offline(path: PathBuf, key: Option<String>, ctx: &AppContext) -> Result<()> {
    let certificate = fs::read_to_string(&path).with_context(|| {
        format!(
            "Failed to read the offline license certificate at {}",
            path.display()
        )
    })?;

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let licensing = client.import_offline_license(ImportOfflineLicenseRequest {
            certificate,
            key: key
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })?;
        print_licensing_summary("Imported offline license", &licensing);
        return Ok(());
    }

    let config = ctx.config_store.load()?;
    let key = key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            config
                .licensing
                .license
                .as_ref()
                .map(|license| license.key.clone())
        })
        .ok_or_else(|| {
            anyhow::anyhow!("A license key is required to import an offline Keygen license file.")
        })?;
    let (license, offline_license) =
        build_offline_imported_license(&config.licensing.keygen, &certificate, &key)?;
    ctx.config_store
        .store_licensing(Some(license.clone()), Some(offline_license.clone()))?;

    println!(
        "Imported offline license for {}",
        mask_license_key(&license.key)
    );
    println!("Algorithm: {}", offline_license.algorithm);
    if let Some(expiry) = offline_license.expiry.as_deref() {
        println!("Offline file expiry: {expiry}");
    }

    Ok(())
}

pub fn export_offline(path: PathBuf, ctx: &AppContext) -> Result<()> {
    let config = ctx.config_store.load()?;
    let offline_license = config
        .licensing
        .offline_license
        .ok_or_else(|| anyhow::anyhow!("No cached offline license file is available."))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create the destination directory for {}",
                path.display()
            )
        })?;
    }
    fs::write(&path, offline_license.certificate).with_context(|| {
        format!(
            "Failed to export the offline license certificate to {}",
            path.display()
        )
    })?;
    println!("Exported offline license to {}", path.display());
    Ok(())
}

fn format_limit(value: Option<usize>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unlimited".into())
}

fn print_licensing_summary(action: &str, licensing: &LicensingDto) {
    println!("{action}: {}", licensing.status);
    println!("Plan: {}", licensing.limits.plan);
    if let Some(error) = licensing.last_error.as_deref()
        && !error.trim().is_empty()
    {
        println!("Last error: {error}");
    }
}
