#![allow(dead_code)]

use std::process::Command;

#[cfg(target_os = "linux")]
use std::fs;

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, generic_array::GenericArray};
use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use reqwest::Method;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::{Rfc2822, Rfc3339};

use crate::config::{KeygenConfig, LicensingConfig, OfflineLicenseFile, StoredLicense};

pub const FREE_MAX_PROJECTS: usize = 5;
const KEYGEN_SIGNATURE_HEADER: &str = "Keygen-Signature";
const KEYGEN_SIGNATURE_ACCEPT_HEADER: &str = r#"algorithm="ed25519""#;
const KEYGEN_EXPECTED_SIGNATURE_ALGORITHM: &str = "ed25519";
const KEYGEN_OFFLINE_ALGORITHM: &str = "aes-256-gcm+ed25519";
const KEYGEN_PLAINTEXT_OFFLINE_ALGORITHM: &str = "base64+ed25519";
const KEYGEN_OFFLINE_ALGORITHM_QUERY: &str = "aes-256-gcm%2Bed25519";
const KEYGEN_OFFLINE_TTL_SECONDS: u64 = 2_629_746;
const KEYGEN_SIGNATURE_TTL_SECONDS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLimits {
    pub plan: String,
    pub max_projects: Option<usize>,
    pub max_domains_per_project: Option<usize>,
}

impl PlanLimits {
    pub fn free() -> Self {
        Self {
            plan: "Free".into(),
            max_projects: Some(FREE_MAX_PROJECTS),
            max_domains_per_project: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineFingerprint {
    pub fingerprint: String,
    pub name: Option<String>,
    pub hostname: Option<String>,
    pub platform: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedLicenseStatus {
    NotConfigured,
    Unlicensed,
    Licensed,
    LicensedOffline,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct ResolvedLicenseState {
    pub status: ResolvedLicenseStatus,
    pub limits: PlanLimits,
    pub active_license: Option<StoredLicense>,
    pub offline_license: Option<VerifiedOfflineLicense>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LicenseValidation {
    pub valid: bool,
    pub code: String,
    pub detail: String,
    pub license: StoredLicense,
    pub offline_license: Option<OfflineLicenseFile>,
}

#[derive(Debug, Clone)]
pub struct VerifiedOfflineLicense {
    pub license_id: String,
    pub name: Option<String>,
    pub license_expiry: Option<String>,
    pub file_expiry: Option<String>,
    pub algorithm: String,
    pub plan: Option<String>,
    pub max_projects: Option<usize>,
    pub max_domains_per_project: Option<usize>,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
struct MachineActivation {
    machine_id: String,
}

#[derive(Debug, Clone)]
struct KeygenHttpResponse {
    status: StatusCode,
    body: String,
    signature_verified: bool,
    signature_date: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum KeygenAuth<'a> {
    License(&'a str),
}

pub struct KeygenClient {
    http: Client,
}

impl KeygenClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(format!(
                "{}/{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .context("Failed to build the Keygen HTTP client")?;

        Ok(Self { http })
    }

    /// Validates a license key with an optional device fingerprint scope so
    /// Localify can enforce Keygen's node-locked and floating policies.
    pub fn validate_license_key(
        &self,
        config: &KeygenConfig,
        key: &str,
        machine: Option<&MachineFingerprint>,
    ) -> Result<LicenseValidation> {
        ensure_keygen_is_configured(config)?;

        let url = format!(
            "{}/v1/accounts/{}/licenses/actions/validate-key",
            config.effective_api_url().trim_end_matches('/'),
            config.effective_account()
        );

        let response = self.send_json_request(
            config,
            Method::POST,
            &url,
            None,
            Some(json!({ "meta": validation_meta(config, key, machine) })),
        )?;

        parse_validation_response(response, key, machine)
    }

    /// Validates the license, activates the current device when required and
    /// checks out an encrypted offline license file for later offline use.
    pub fn validate_and_activate_license_key(
        &self,
        config: &KeygenConfig,
        key: &str,
        machine: &MachineFingerprint,
    ) -> Result<LicenseValidation> {
        let validation = self.validate_license_key(config, key, Some(machine))?;
        if !validation.valid {
            if !should_activate_machine(&validation.code) {
                return Ok(validation);
            }

            let activation =
                self.activate_machine(config, key, &validation.license.license_id, machine)?;
            let mut validation = self.validate_license_key(config, key, Some(machine))?;
            validation.license.machine_id = Some(activation.machine_id);
            return self.attach_offline_license(config, key, validation);
        }

        self.attach_offline_license(config, key, validation)
    }

    pub fn checkout_offline_license_file(
        &self,
        config: &KeygenConfig,
        key: &str,
        license_id: &str,
    ) -> Result<OfflineLicenseFile> {
        ensure_keygen_is_configured(config)?;

        let url = format!(
            "{}/v1/accounts/{}/licenses/{}/actions/check-out?ttl={}&algorithm={}",
            config.effective_api_url().trim_end_matches('/'),
            config.effective_account(),
            license_id.trim(),
            KEYGEN_OFFLINE_TTL_SECONDS,
            KEYGEN_OFFLINE_ALGORITHM_QUERY
        );

        let response = self.send_json_request(
            config,
            Method::POST,
            &url,
            Some(KeygenAuth::License(key)),
            None,
        )?;

        if !response.status.is_success() {
            return Err(anyhow::anyhow!(format_keygen_error(
                response.status.as_u16(),
                &response.body
            )));
        }

        let payload: KeygenCheckoutResponse = serde_json::from_str(&response.body)
            .context("Failed to parse the Keygen checkout response")?;

        Ok(OfflineLicenseFile {
            certificate: payload.data.attributes.certificate,
            algorithm: payload.data.attributes.algorithm,
            issued: payload.data.attributes.issued,
            expiry: payload.data.attributes.expiry,
        })
    }

    fn attach_offline_license(
        &self,
        config: &KeygenConfig,
        key: &str,
        mut validation: LicenseValidation,
    ) -> Result<LicenseValidation> {
        if !validation.valid || configured_public_key(config).is_none() {
            return Ok(validation);
        }

        let offline_license =
            self.checkout_offline_license_file(config, key, &validation.license.license_id)?;
        validation.offline_license = Some(offline_license);

        Ok(validation)
    }

    fn activate_machine(
        &self,
        config: &KeygenConfig,
        key: &str,
        license_id: &str,
        machine: &MachineFingerprint,
    ) -> Result<MachineActivation> {
        ensure_keygen_is_configured(config)?;

        let url = format!(
            "{}/v1/accounts/{}/machines",
            config.effective_api_url().trim_end_matches('/'),
            config.effective_account()
        );

        let response = self.send_json_request(
            config,
            Method::POST,
            &url,
            Some(KeygenAuth::License(key)),
            Some(json!({
                "data": {
                    "type": "machines",
                    "attributes": {
                        "fingerprint": machine.fingerprint.clone(),
                        "platform": machine.platform.clone(),
                        "name": machine.name.clone(),
                        "hostname": machine.hostname.clone(),
                    },
                    "relationships": {
                        "license": {
                            "data": {
                                "type": "licenses",
                                "id": license_id,
                            }
                        }
                    }
                }
            })),
        )?;

        if response.status != StatusCode::CREATED && response.status != StatusCode::OK {
            return Err(anyhow::anyhow!(format_keygen_error(
                response.status.as_u16(),
                &response.body
            )));
        }

        let payload: KeygenMachineResponse = serde_json::from_str(&response.body)
            .context("Failed to parse the Keygen activation response")?;

        Ok(MachineActivation {
            machine_id: payload.data.id,
        })
    }

    fn send_json_request(
        &self,
        config: &KeygenConfig,
        method: Method,
        url: &str,
        auth: Option<KeygenAuth<'_>>,
        body: Option<Value>,
    ) -> Result<KeygenHttpResponse> {
        let parsed_url = reqwest::Url::parse(url)
            .with_context(|| format!("Failed to parse Keygen URL '{url}'"))?;

        let mut request = self
            .http
            .request(method.clone(), parsed_url.clone())
            .header("Accept", "application/vnd.api+json");

        if body.is_some() {
            request = request.header("Content-Type", "application/vnd.api+json");
        }

        if let Some(auth) = auth {
            request = match auth {
                KeygenAuth::License(key) => {
                    request.header("Authorization", format!("License {}", key.trim()))
                }
            };
        }

        if configured_public_key(config).is_some() {
            request = request.header("Keygen-Accept-Signature", KEYGEN_SIGNATURE_ACCEPT_HEADER);
        }

        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().context("Failed to reach Keygen")?;
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response
            .bytes()
            .context("Failed to read the Keygen response")?;

        let mut signature_verified = false;
        let mut signature_date = None;
        if status.is_success()
            && let Some(public_key) = configured_public_key(config)
        {
            signature_date = Some(verify_response_signature(
                &method,
                &parsed_url,
                &headers,
                bytes.as_ref(),
                public_key,
            )?);
            signature_verified = true;
        }

        let body =
            String::from_utf8(bytes.to_vec()).context("Keygen returned a non UTF-8 response")?;

        Ok(KeygenHttpResponse {
            status,
            body,
            signature_verified,
            signature_date,
        })
    }
}

/// Returns the current device fingerprint that Localify scopes Keygen
/// validations against so seat and machine limits stay consistent.
pub fn current_machine_fingerprint() -> Result<MachineFingerprint> {
    let raw_identifier = current_machine_identifier()
        .context("Failed to resolve a stable machine identifier for Keygen")?;

    Ok(MachineFingerprint {
        fingerprint: sha256_hex(raw_identifier.as_bytes()),
        name: current_machine_name().ok(),
        hostname: current_hostname().ok(),
        platform: current_platform_label().into(),
    })
}

pub fn resolve_license_state(licensing: &LicensingConfig) -> ResolvedLicenseState {
    let configured = licensing.keygen.is_configured();
    let stored_license = licensing.license.clone();
    let stored_valid = stored_license
        .as_ref()
        .and_then(|value| value.validation_code.as_deref())
        == Some("VALID");

    let offline_result = verify_offline_license_cache(licensing);
    let offline_license = offline_result.as_ref().ok().cloned();
    let offline_error = if licensing.offline_license.is_some() {
        offline_result.err().map(|error| error.to_string())
    } else {
        None
    };
    let uses_verified_offline_cache =
        configured_public_key(&licensing.keygen).is_some() && licensing.offline_license.is_some();

    if let Some(offline_license) = offline_license {
        let status = if stored_valid {
            ResolvedLicenseStatus::Licensed
        } else {
            ResolvedLicenseStatus::LicensedOffline
        };

        return ResolvedLicenseState {
            status,
            limits: PlanLimits {
                plan: offline_license
                    .plan
                    .clone()
                    .unwrap_or_else(|| "Licensed".into()),
                max_projects: offline_license.max_projects,
                max_domains_per_project: offline_license.max_domains_per_project,
            },
            active_license: stored_license,
            offline_license: Some(offline_license),
            last_error: None,
        };
    }

    if uses_verified_offline_cache {
        return ResolvedLicenseState {
            status: if configured {
                ResolvedLicenseStatus::Invalid
            } else {
                ResolvedLicenseStatus::NotConfigured
            },
            limits: PlanLimits::free(),
            active_license: stored_license,
            offline_license: None,
            last_error: offline_error,
        };
    }

    if stored_valid {
        return ResolvedLicenseState {
            status: ResolvedLicenseStatus::Licensed,
            limits: plan_limits_from_stored_license(stored_license.as_ref()),
            active_license: stored_license,
            offline_license: None,
            last_error: None,
        };
    }

    let status = if stored_license.is_some() {
        ResolvedLicenseStatus::Invalid
    } else if configured {
        ResolvedLicenseStatus::Unlicensed
    } else {
        ResolvedLicenseStatus::NotConfigured
    };

    ResolvedLicenseState {
        status,
        limits: PlanLimits::free(),
        active_license: stored_license,
        offline_license: None,
        last_error: offline_error,
    }
}

pub fn plan_limits_for_license(licensing: &LicensingConfig) -> PlanLimits {
    resolve_license_state(licensing).limits
}

pub fn verify_offline_license_certificate(
    config: &KeygenConfig,
    certificate: &str,
    key: &str,
) -> Result<VerifiedOfflineLicense> {
    let public_key = configured_public_key(config)
        .ok_or_else(|| anyhow::anyhow!("A Keygen public key is required for offline licensing."))?;

    let payload = decode_certificate_payload(certificate)?;
    let algorithm = payload.alg.trim();
    if algorithm != KEYGEN_OFFLINE_ALGORITHM && algorithm != KEYGEN_PLAINTEXT_OFFLINE_ALGORITHM {
        bail!(
            "Unsupported Keygen offline license algorithm '{algorithm}'. Expected '{KEYGEN_OFFLINE_ALGORITHM}' or '{KEYGEN_PLAINTEXT_OFFLINE_ALGORITHM}'."
        );
    }

    verify_ed25519_signature(
        public_key,
        &payload.sig,
        format!("license/{}", payload.enc).as_bytes(),
    )
    .context("Offline license file signature verification failed")?;

    let plaintext = decrypt_or_decode_license_payload(&payload, key)?;
    let dataset: Value = serde_json::from_slice(&plaintext)
        .context("Failed to parse the offline license dataset")?;

    if let Some(file_expiry) = first_string(
        &dataset,
        &["/meta/expiry", "/expiry", "/data/attributes/expiry"],
    ) {
        ensure_timestamp_not_expired(&file_expiry)
            .with_context(|| "Offline license file has expired".to_string())?;
    }

    let metadata = first_value(
        &dataset,
        &[
            "/data/attributes/metadata",
            "/license/metadata",
            "/metadata",
        ],
    )
    .cloned()
    .unwrap_or_else(|| json!({}));

    let license_id = first_string(&dataset, &["/data/id", "/license/id", "/id"])
        .ok_or_else(|| anyhow::anyhow!("Offline license dataset did not contain a license id."))?;

    Ok(VerifiedOfflineLicense {
        license_id,
        name: first_string(&dataset, &["/data/attributes/name", "/license/name"]),
        license_expiry: first_string(&dataset, &["/data/attributes/expiry", "/license/expiry"]),
        file_expiry: first_string(&dataset, &["/meta/expiry", "/expiry"]),
        algorithm: payload.alg,
        plan: extract_plan_name(&metadata),
        max_projects: extract_optional_limit(&metadata, "maxProjects"),
        max_domains_per_project: extract_optional_limit(&metadata, "maxDomainsPerProject"),
        metadata,
    })
}

pub fn build_offline_imported_license(
    config: &KeygenConfig,
    certificate: &str,
    key: &str,
) -> Result<(StoredLicense, OfflineLicenseFile)> {
    let verified = verify_offline_license_certificate(config, certificate, key)?;

    let offline_license = OfflineLicenseFile {
        certificate: certificate.trim().to_string(),
        algorithm: verified.algorithm.clone(),
        issued: None,
        expiry: verified.file_expiry.clone(),
    };

    Ok((
        StoredLicense {
            key: key.trim().to_string(),
            license_id: verified.license_id,
            name: verified.name,
            expiry: verified.license_expiry,
            plan: verified.plan.clone(),
            max_projects: verified.max_projects,
            max_domains_per_project: verified.max_domains_per_project,
            validation_code: Some("VALID_OFFLINE".into()),
            validation_detail: Some("Validated from an offline Keygen license file.".into()),
            last_validated_at: Some(current_timestamp()),
            machine_id: None,
            machine_fingerprint: None,
            machine_name: None,
            machine_hostname: None,
            machine_platform: None,
            response_signature_verified: false,
            response_signature_date: None,
        },
        offline_license,
    ))
}

pub fn mask_license_key(key: &str) -> String {
    mask_value(key, 4, 4)
}

pub fn mask_fingerprint(fingerprint: &str) -> String {
    mask_value(fingerprint, 8, 6)
}

fn plan_limits_from_stored_license(license: Option<&StoredLicense>) -> PlanLimits {
    let Some(license) = license else {
        return PlanLimits::free();
    };

    PlanLimits {
        plan: license.plan.clone().unwrap_or_else(|| "Licensed".into()),
        max_projects: license.max_projects,
        max_domains_per_project: license.max_domains_per_project,
    }
}

fn verify_offline_license_cache(licensing: &LicensingConfig) -> Result<VerifiedOfflineLicense> {
    let Some(offline_license) = licensing.offline_license.as_ref() else {
        bail!("No cached offline license file is available.");
    };
    let Some(stored_license) = licensing.license.as_ref() else {
        bail!("No stored license key is available to decrypt the offline license file.");
    };

    verify_offline_license_certificate(
        &licensing.keygen,
        &offline_license.certificate,
        &stored_license.key,
    )
}

fn validation_meta(
    config: &KeygenConfig,
    key: &str,
    machine: Option<&MachineFingerprint>,
) -> serde_json::Map<String, Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("key".into(), Value::String(key.trim().to_string()));
    meta.insert(
        "nonce".into(),
        Value::Number(serde_json::Number::from(
            OffsetDateTime::now_utc().unix_timestamp(),
        )),
    );

    let mut scope = serde_json::Map::new();
    if let Some(product) = config.effective_product() {
        scope.insert("product".into(), Value::String(product.to_string()));
    }
    if let Some(machine) = machine {
        scope.insert(
            "fingerprint".into(),
            Value::String(machine.fingerprint.clone()),
        );
    }
    if !scope.is_empty() {
        meta.insert("scope".into(), Value::Object(scope));
    }

    meta
}

fn parse_validation_response(
    response: KeygenHttpResponse,
    key: &str,
    machine: Option<&MachineFingerprint>,
) -> Result<LicenseValidation> {
    if !response.status.is_success() {
        return Err(anyhow::anyhow!(format_keygen_error(
            response.status.as_u16(),
            &response.body
        )));
    }

    let payload: KeygenValidationResponse =
        serde_json::from_str(&response.body).context("Failed to parse the Keygen response")?;

    let Some(license) = payload.data else {
        bail!("Keygen returned a validation response without a license payload.");
    };

    let code = payload.meta.code;
    let detail = payload.meta.detail;

    Ok(LicenseValidation {
        valid: payload.meta.valid,
        code: code.clone(),
        detail: detail.clone(),
        license: StoredLicense {
            key: key.trim().to_string(),
            license_id: license.id,
            name: license.attributes.name,
            expiry: license.attributes.expiry,
            plan: extract_plan_name(&license.attributes.metadata),
            max_projects: extract_optional_limit(&license.attributes.metadata, "maxProjects"),
            max_domains_per_project: extract_optional_limit(
                &license.attributes.metadata,
                "maxDomainsPerProject",
            ),
            validation_code: Some(code),
            validation_detail: Some(detail),
            last_validated_at: Some(current_timestamp()),
            machine_id: None,
            machine_fingerprint: machine.map(|value| value.fingerprint.clone()),
            machine_name: machine.and_then(|value| value.name.clone()),
            machine_hostname: machine.and_then(|value| value.hostname.clone()),
            machine_platform: machine.map(|value| value.platform.clone()),
            response_signature_verified: response.signature_verified,
            response_signature_date: response.signature_date,
        },
        offline_license: None,
    })
}

fn ensure_keygen_is_configured(config: &KeygenConfig) -> Result<()> {
    if config.effective_account().trim().is_empty() {
        bail!("Keygen account is not configured.");
    }

    if config.effective_api_url().trim().is_empty() {
        bail!("Keygen API URL is not configured.");
    }

    Ok(())
}

fn should_activate_machine(code: &str) -> bool {
    matches!(
        code,
        "FINGERPRINT_SCOPE_MISMATCH" | "NO_MACHINE" | "NO_MACHINES"
    )
}

fn verify_response_signature(
    method: &Method,
    url: &reqwest::Url,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
    public_key: &str,
) -> Result<String> {
    let signature_header = header_value(headers, KEYGEN_SIGNATURE_HEADER)
        .ok_or_else(|| anyhow::anyhow!("Missing Keygen response signature header."))?;
    let digest_header = header_value(headers, "Digest")
        .ok_or_else(|| anyhow::anyhow!("Missing Keygen digest header."))?;
    let date_header = header_value(headers, "Date")
        .ok_or_else(|| anyhow::anyhow!("Missing Keygen response date header."))?;

    let expected_digest = format!("sha-256={}", BASE64.encode(Sha256::digest(body)));
    if digest_header != expected_digest {
        bail!("Keygen response digest mismatch.");
    }

    let response_time = OffsetDateTime::parse(&date_header, &Rfc2822)
        .context("Failed to parse the Keygen response date header")?;
    let now = OffsetDateTime::now_utc();
    let skew = (now - response_time).whole_seconds().abs();
    if skew > KEYGEN_SIGNATURE_TTL_SECONDS {
        bail!("Keygen response signature is stale. Check the system clock and retry.");
    }

    let signature = parse_signature_header(&signature_header)?;
    if signature.algorithm != KEYGEN_EXPECTED_SIGNATURE_ALGORITHM {
        bail!(
            "Unexpected Keygen response signature algorithm '{}'.",
            signature.algorithm
        );
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Keygen URL did not contain a host."))?;
    let mut request_target = url.path().to_string();
    if let Some(query) = url.query() {
        request_target.push('?');
        request_target.push_str(query);
    }

    let signing_data = format!(
        "(request-target): {} {}\nhost: {}\ndate: {}\ndigest: {}",
        method.as_str().to_ascii_lowercase(),
        request_target,
        host.to_ascii_lowercase(),
        date_header,
        digest_header
    );

    verify_ed25519_signature(public_key, &signature.signature, signing_data.as_bytes())
        .context("Failed to verify the Keygen response signature")?;

    response_time
        .format(&Rfc3339)
        .context("Failed to format the verified Keygen response time")
}

fn verify_ed25519_signature(
    public_key_hex: &str,
    signature_b64: &str,
    message: &[u8],
) -> Result<()> {
    let public_key_bytes = decode_hex(public_key_hex.trim())
        .context("Keygen public key must be hex-encoded Ed25519 bytes")?;
    let public_key: [u8; 32] = public_key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Keygen public key must be exactly 32 bytes long."))?;
    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .context("Failed to construct the Keygen verifying key")?;

    let signature_bytes = BASE64
        .decode(signature_b64.trim())
        .context("Failed to decode the Keygen signature")?;
    let signature = Signature::from_slice(&signature_bytes)
        .context("Failed to parse the Keygen signature bytes")?;

    verifying_key.verify(message, &signature)?;
    Ok(())
}

fn decode_certificate_payload(certificate: &str) -> Result<KeygenOfflineCertificatePayload> {
    let normalized = certificate
        .trim()
        .lines()
        .filter(|line| !line.starts_with("-----BEGIN ") && !line.starts_with("-----END "))
        .map(str::trim)
        .collect::<String>();

    if normalized.is_empty() {
        bail!("Offline license certificate is empty.");
    }

    let payload = BASE64
        .decode(normalized.as_bytes())
        .context("Failed to decode the offline license certificate payload")?;

    serde_json::from_slice(&payload).context("Failed to parse the offline license certificate")
}

fn decrypt_or_decode_license_payload(
    payload: &KeygenOfflineCertificatePayload,
    key: &str,
) -> Result<Vec<u8>> {
    match payload.alg.as_str() {
        KEYGEN_PLAINTEXT_OFFLINE_ALGORITHM => BASE64
            .decode(payload.enc.as_bytes())
            .context("Failed to decode the plaintext offline license payload"),
        KEYGEN_OFFLINE_ALGORITHM => {
            let parts = payload.enc.split('.').collect::<Vec<_>>();
            if parts.len() != 3 {
                bail!("Encrypted offline license payload is malformed.");
            }

            let ciphertext = BASE64
                .decode(parts[0].as_bytes())
                .context("Failed to decode offline license ciphertext")?;
            let iv = BASE64
                .decode(parts[1].as_bytes())
                .context("Failed to decode offline license IV")?;
            let tag = BASE64
                .decode(parts[2].as_bytes())
                .context("Failed to decode offline license auth tag")?;

            if iv.len() != 12 {
                bail!("Offline license IV must be 12 bytes long.");
            }
            if tag.len() != 16 {
                bail!("Offline license auth tag must be 16 bytes long.");
            }

            let secret = Sha256::digest(key.trim().as_bytes());
            let cipher = Aes256Gcm::new(GenericArray::from_slice(secret.as_slice()));
            let nonce = GenericArray::from_slice(iv.as_slice());
            let mut encrypted = ciphertext;
            encrypted.extend(tag);

            cipher
                .decrypt(nonce, encrypted.as_ref())
                .map_err(|_| anyhow::anyhow!("Failed to decrypt the offline license payload"))
        }
        other => bail!("Unsupported offline license algorithm '{other}'."),
    }
}

fn configured_public_key(config: &KeygenConfig) -> Option<&str> {
    config.effective_public_key()
}

fn current_machine_identifier() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos_machine_identifier()
    }

    #[cfg(target_os = "linux")]
    {
        linux_machine_identifier()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        current_hostname()
    }
}

#[cfg(target_os = "macos")]
fn macos_machine_identifier() -> Result<String> {
    let output = run_command("ioreg", &["-rd1", "-c", "IOPlatformExpertDevice"])
        .context("Failed to query IOPlatformUUID with ioreg")?;

    parse_ioplatform_uuid(&output)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("IOPlatformUUID was not present in ioreg output"))
}

#[cfg(target_os = "linux")]
fn linux_machine_identifier() -> Result<String> {
    let machine_id = fs::read_to_string("/etc/machine-id")
        .or_else(|_| fs::read_to_string("/var/lib/dbus/machine-id"))
        .context("Failed to read the Linux machine-id file")?;

    let machine_id = machine_id.trim();
    if machine_id.is_empty() {
        bail!("Linux machine-id file is empty.");
    }

    Ok(machine_id.to_string())
}

fn current_machine_name() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let value = run_command("scutil", &["--get", "ComputerName"])?;
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }

    current_hostname()
}

fn current_hostname() -> Result<String> {
    if let Some(hostname) = std::env::var_os("HOSTNAME")
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(hostname);
    }

    if let Some(hostname) = std::env::var_os("COMPUTERNAME")
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(hostname);
    }

    let output = run_command("hostname", &[]).context("Failed to resolve the current hostname")?;
    let hostname = output.trim();
    if hostname.is_empty() {
        bail!("hostname returned an empty value.");
    }

    Ok(hostname.to_string())
}

fn current_platform_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS"
    }

    #[cfg(target_os = "linux")]
    {
        "Linux"
    }

    #[cfg(target_os = "windows")]
    {
        "Windows"
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        std::env::consts::OS
    }
}

fn run_command(command: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("Failed to spawn '{command}'"))?;

    if !output.status.success() {
        bail!(
            "'{command}' exited with status {}",
            output.status.code().unwrap_or_default()
        );
    }

    String::from_utf8(output.stdout)
        .with_context(|| format!("'{command}' returned invalid UTF-8 output"))
}

fn parse_ioplatform_uuid(output: &str) -> Option<&str> {
    output
        .lines()
        .find(|line| line.contains("IOPlatformUUID"))
        .and_then(|line| line.split('"').nth(3))
        .filter(|value| !value.trim().is_empty())
}

fn parse_signature_header(header: &str) -> Result<KeygenSignatureHeader> {
    let mut keyid = None;
    let mut algorithm = None;
    let mut signature = None;
    let mut headers = None;

    for part in header.split(',') {
        let mut pieces = part.trim().splitn(2, '=');
        let Some(name) = pieces.next().map(str::trim) else {
            continue;
        };
        let Some(raw_value) = pieces.next().map(str::trim) else {
            continue;
        };
        let value = raw_value.trim_matches('"').to_string();

        match name {
            "keyid" => keyid = Some(value),
            "algorithm" => algorithm = Some(value),
            "signature" => signature = Some(value),
            "headers" => headers = Some(value),
            _ => {}
        }
    }

    let result = KeygenSignatureHeader {
        keyid: keyid.ok_or_else(|| anyhow::anyhow!("Missing Keygen signature key id."))?,
        algorithm: algorithm
            .ok_or_else(|| anyhow::anyhow!("Missing Keygen signature algorithm."))?,
        signature: signature.ok_or_else(|| anyhow::anyhow!("Missing Keygen signature value."))?,
        headers: headers.ok_or_else(|| anyhow::anyhow!("Missing Keygen signed headers."))?,
    };

    if result.headers != "(request-target) host date digest" {
        bail!("Unexpected Keygen signed headers '{}'.", result.headers);
    }

    Ok(result)
}

fn header_value(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn first_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn first_value<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a Value> {
    pointers.iter().find_map(|pointer| value.pointer(pointer))
}

fn ensure_timestamp_not_expired(value: &str) -> Result<()> {
    let expiry = OffsetDateTime::parse(value, &Rfc3339)
        .with_context(|| format!("Failed to parse timestamp '{value}'"))?;
    if OffsetDateTime::now_utc() > expiry {
        bail!("Timestamp is in the past.");
    }

    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    let trimmed = value.trim();
    if !trimmed.len().is_multiple_of(2) {
        bail!("Hex input must be an even number of characters.");
    }

    trimmed
        .as_bytes()
        .chunks(2)
        .map(|chunk| {
            let high = decode_hex_nibble(chunk[0])?;
            let low = decode_hex_nibble(chunk[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("Invalid hex character '{}'.", value as char),
    }
}

fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(digest.len() * 2);

    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }

    output
}

fn extract_plan_name(metadata: &Value) -> Option<String> {
    metadata
        .as_object()
        .and_then(|value| value.get("plan"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn extract_optional_limit(metadata: &Value, key: &str) -> Option<usize> {
    metadata
        .as_object()
        .and_then(|value| value.get(key))
        .and_then(|value| match value {
            Value::Null => None,
            Value::Number(number) => number
                .as_u64()
                .and_then(|value| usize::try_from(value).ok()),
            _ => None,
        })
}

fn current_timestamp() -> String {
    OffsetDateTime::now_utc().unix_timestamp().to_string()
}

fn format_keygen_error(status: u16, body: &str) -> String {
    let parsed = serde_json::from_str::<KeygenErrorResponse>(body).ok();
    let details = parsed
        .and_then(|payload| payload.errors.into_iter().next())
        .map(|error| match error.detail {
            Some(detail) if !detail.trim().is_empty() => detail,
            _ => error.title,
        });

    match details {
        Some(detail) => format!("Keygen request failed ({status}): {detail}"),
        None => format!("Keygen request failed ({status})."),
    }
}

fn mask_value(value: &str, prefix: usize, suffix: usize) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= prefix + suffix {
        return trimmed.to_string();
    }

    format!(
        "{}…{}",
        &trimmed[..prefix],
        &trimmed[trimmed.len() - suffix..]
    )
}

#[derive(Debug, Deserialize)]
struct KeygenValidationResponse {
    meta: KeygenValidationMeta,
    data: Option<KeygenLicenseData>,
}

#[derive(Debug, Deserialize)]
struct KeygenValidationMeta {
    valid: bool,
    code: String,
    detail: String,
}

#[derive(Debug, Deserialize)]
struct KeygenLicenseData {
    id: String,
    attributes: KeygenLicenseAttributes,
}

#[derive(Debug, Deserialize)]
struct KeygenLicenseAttributes {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    expiry: Option<String>,
    #[serde(default)]
    metadata: Value,
}

#[derive(Debug, Deserialize)]
struct KeygenMachineResponse {
    data: KeygenMachineData,
}

#[derive(Debug, Deserialize)]
struct KeygenMachineData {
    id: String,
}

#[derive(Debug, Deserialize)]
struct KeygenCheckoutResponse {
    data: KeygenCheckoutData,
}

#[derive(Debug, Deserialize)]
struct KeygenCheckoutData {
    attributes: KeygenCheckoutAttributes,
}

#[derive(Debug, Deserialize)]
struct KeygenCheckoutAttributes {
    certificate: String,
    algorithm: String,
    #[serde(default)]
    issued: Option<String>,
    #[serde(default)]
    expiry: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KeygenOfflineCertificatePayload {
    enc: String,
    sig: String,
    alg: String,
}

#[derive(Debug)]
struct KeygenSignatureHeader {
    keyid: String,
    algorithm: String,
    signature: String,
    headers: String,
}

#[derive(Debug, Deserialize)]
struct KeygenErrorResponse {
    #[serde(default)]
    errors: Vec<KeygenErrorItem>,
}

#[derive(Debug, Deserialize)]
struct KeygenErrorItem {
    title: String,
    #[serde(default)]
    detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        KEYGEN_OFFLINE_ALGORITHM, KeygenHttpResponse, ResolvedLicenseStatus, decode_hex,
        mask_fingerprint, parse_ioplatform_uuid, parse_signature_header, parse_validation_response,
        resolve_license_state, should_activate_machine,
    };
    use crate::config::{LicensingConfig, OfflineLicenseFile, StoredLicense};
    use reqwest::StatusCode;

    #[test]
    fn parses_ioplatform_uuid_from_ioreg_output() {
        let output = r#"
            | |   "IOPlatformUUID" = "12345678-1234-1234-1234-1234567890AB"
        "#;

        assert_eq!(
            parse_ioplatform_uuid(output),
            Some("12345678-1234-1234-1234-1234567890AB")
        );
    }

    #[test]
    fn machine_activation_codes_are_detected() {
        assert!(should_activate_machine("FINGERPRINT_SCOPE_MISMATCH"));
        assert!(should_activate_machine("NO_MACHINE"));
        assert!(!should_activate_machine("EXPIRED"));
    }

    #[test]
    fn validation_response_accepts_null_metadata() {
        let response = KeygenHttpResponse {
            status: StatusCode::OK,
            body: serde_json::json!({
                "meta": {
                    "valid": true,
                    "code": "VALID",
                    "detail": "is valid"
                },
                "data": {
                    "id": "lic_123",
                    "attributes": {
                        "name": "Pro",
                        "metadata": null
                    }
                }
            })
            .to_string(),
            signature_verified: false,
            signature_date: None,
        };

        let validation = parse_validation_response(response, "TEST-KEY-1234", None).unwrap();

        assert!(validation.valid);
        assert_eq!(validation.license.license_id, "lic_123");
        assert_eq!(validation.license.plan, None);
        assert_eq!(validation.license.max_projects, None);
        assert_eq!(validation.license.max_domains_per_project, None);
    }

    #[test]
    fn fingerprint_mask_keeps_prefix_and_suffix() {
        assert_eq!(
            mask_fingerprint("0123456789abcdef0123456789abcdef"),
            "01234567…abcdef"
        );
    }

    #[test]
    fn decodes_hex_public_key() {
        assert_eq!(decode_hex("0aFF").unwrap(), vec![10, 255]);
    }

    #[test]
    fn parses_signature_header_components() {
        let parsed = parse_signature_header(
            r#"keyid="abc", algorithm="ed25519", signature="xyz", headers="(request-target) host date digest""#,
        )
        .unwrap();

        assert_eq!(parsed.keyid, "abc");
        assert_eq!(parsed.algorithm, "ed25519");
        assert_eq!(parsed.signature, "xyz");
        assert_eq!(parsed.headers, "(request-target) host date digest");
    }

    #[test]
    fn offline_algorithm_constant_is_encrypted_ed25519() {
        assert_eq!(KEYGEN_OFFLINE_ALGORITHM, "aes-256-gcm+ed25519");
    }

    #[test]
    fn missing_offline_cache_is_not_a_visible_license_error() {
        let licensing = LicensingConfig::default();

        let resolved = resolve_license_state(&licensing);

        assert_eq!(resolved.status, ResolvedLicenseStatus::Unlicensed);
        assert_eq!(resolved.last_error, None);
    }

    #[test]
    fn invalid_present_offline_cache_remains_a_visible_license_error() {
        let licensing = LicensingConfig {
            license: Some(StoredLicense {
                key: "localify-test-key".into(),
                ..StoredLicense::default()
            }),
            offline_license: Some(OfflineLicenseFile {
                certificate: "not-a-valid-offline-certificate".into(),
                ..OfflineLicenseFile::default()
            }),
            ..LicensingConfig::default()
        };

        let resolved = resolve_license_state(&licensing);

        assert_eq!(resolved.status, ResolvedLicenseStatus::Invalid);
        assert!(resolved.last_error.is_some());
    }
}
