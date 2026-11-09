//! What a check reads: the manifest the app's updater reads (`latest.json`),
//! and the signed archive it names. The manifest is discovery, not
//! authority: only the archive's minisign signature, checked against the
//! updater key built into Localify, makes it trusted.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Bytes;
use hyper::header::USER_AGENT;
use hyper::{Request, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::rules::parse_version;
use crate::infrastructure::cloud::http::block_on;

/// The updater's public key, the one the app's updater checks with.
const UPDATER_PUBLIC_KEY: &str = include_str!("../../../src-tauri/updater.pub");
/// Set at build time, as for the app (`src-tauri/build.rs`).
const UPDATER_ENDPOINT: Option<&str> = option_env!("LOCALIFY_UPDATER_ENDPOINT");
const UPDATER_BASE_URL: Option<&str> = option_env!("LOCALIFY_UPDATER_BASE_URL");

const MANIFEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A release archive is about 15 MB.
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

/// The updater key built into this binary, base64 as in `updater.pub`.
pub fn updater_public_key() -> &'static str {
    UPDATER_PUBLIC_KEY.trim()
}

/// The manifest this build checks, or `None` for a debug build or a build
/// made without `LOCALIFY_UPDATER_ENDPOINT` or `LOCALIFY_UPDATER_BASE_URL`,
/// which then never checks.
pub fn updater_endpoint() -> Option<String> {
    if cfg!(debug_assertions) {
        return None;
    }
    endpoint_from(UPDATER_ENDPOINT, UPDATER_BASE_URL)
}

fn endpoint_from(endpoint: Option<&str>, base_url: Option<&str>) -> Option<String> {
    let endpoint = endpoint.map(str::trim).filter(|value| !value.is_empty());
    if let Some(endpoint) = endpoint {
        return Some(endpoint.to_string());
    }
    base_url
        .map(|base| base.trim().trim_end_matches('/'))
        .filter(|base| !base.is_empty())
        .map(|base| format!("{base}/latest.json"))
}

/// The manifest's key for this Mac's archive, as the app's updater names it.
pub fn platform_key() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("darwin-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("darwin-x86_64")
    } else {
        None
    }
}

/// The release a manifest offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    /// The archive for this platform. Linux has none: it only learns that
    /// a version exists.
    #[serde(default)]
    pub archive: Option<ReleaseArchive>,
}

impl Release {
    /// What tells this release apart before its archive is downloaded: its
    /// version and its archive's signature, hashed. A release that failed
    /// here is skipped by it, without a download.
    pub fn identity(&self) -> String {
        let signature = self
            .archive
            .as_ref()
            .map_or("", |archive| archive.signature.as_str());
        sha256_hex(format!("{}\n{signature}", self.version).as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseArchive {
    pub url: String,
    /// Tauri's format: the minisign signature file, base64.
    pub signature: String,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    platforms: HashMap<String, ManifestPlatform>,
}

#[derive(Deserialize)]
struct ManifestPlatform {
    signature: String,
    url: String,
}

/// Reads `latest.json`. With a `platform`, the manifest must name an HTTPS
/// archive for it.
pub fn parse_manifest(bytes: &[u8], platform: Option<&str>) -> Result<Release> {
    let manifest: Manifest =
        serde_json::from_slice(bytes).context("The update manifest is not valid JSON")?;
    let version = parse_version(&manifest.version)
        .ok_or_else(|| anyhow!("The update manifest has an invalid version"))?;
    let archive = match platform {
        None => None,
        Some(key) => {
            let entry = manifest
                .platforms
                .get(key)
                .ok_or_else(|| anyhow!("The update manifest has no archive for {key}"))?;
            if !entry.url.starts_with("https://") {
                bail!("The update manifest names an archive that is not HTTPS");
            }
            Some(ReleaseArchive {
                url: entry.url.clone(),
                signature: entry.signature.trim().to_string(),
            })
        }
    };
    Ok(Release {
        version: version.to_string(),
        archive,
    })
}

/// Reads the manifest at `endpoint` for this platform.
pub fn fetch_manifest(endpoint: &str) -> Result<Release> {
    let bytes = get(endpoint, MAX_MANIFEST_BYTES, MANIFEST_TIMEOUT)
        .with_context(|| format!("Could not read the update manifest at {endpoint}"))?;
    parse_manifest(&bytes, platform_key())
}

/// Downloads an archive whole, capped, and returns it with its SHA-256.
pub fn download_archive(url: &str) -> Result<(Vec<u8>, String)> {
    let bytes = get(url, MAX_ARCHIVE_BYTES, ARCHIVE_TIMEOUT)
        .with_context(|| format!("Could not download the update from {url}"))?;
    let digest = sha256_hex(&bytes);
    Ok((bytes, digest))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Checks `data` against a signature in Tauri's format: the minisign
/// signature file and public key, each base64. Only prehashed signatures,
/// which every Localify release has, are accepted.
pub fn verify_signature(data: &[u8], signature_b64: &str, public_key_b64: &str) -> Result<()> {
    let public_key = decode_text(public_key_b64).context("The updater key is not readable")?;
    let public_key = PublicKey::decode(&public_key)
        .map_err(|error| anyhow!("The updater key is not readable: {error}"))?;
    let signature = decode_text(signature_b64).context("The update's signature is not readable")?;
    let signature = Signature::decode(&signature)
        .map_err(|error| anyhow!("The update's signature is not readable: {error}"))?;
    public_key
        .verify(data, &signature, false)
        .map_err(|error| anyhow!("The update's signature does not match Localify's key: {error}"))
}

fn decode_text(value: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(value.trim())?;
    Ok(String::from_utf8(bytes)?)
}

/// One HTTPS GET, read whole up to `max_bytes`. Redirects are not followed.
fn get(url: &str, max_bytes: usize, timeout: Duration) -> Result<Vec<u8>> {
    let uri: Uri = url.parse().context("invalid URL")?;
    if uri.scheme_str() != Some("https") {
        bail!("only HTTPS is allowed");
    }
    block_on(async move {
        tokio::time::timeout(timeout, get_async(uri, max_bytes))
            .await
            .map_err(|_| anyhow!("no complete answer after {} s", timeout.as_secs()))?
    })?
}

async fn get_async(uri: Uri, max_bytes: usize) -> Result<Vec<u8>> {
    // An explicit provider, because the app never installs a default one.
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider())?
        .https_only()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let request = Request::get(uri)
        .header(USER_AGENT, concat!("localify/", env!("CARGO_PKG_VERSION")))
        .body(Empty::new())?;
    let response = client.request(request).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("the server answered {status}");
    }
    let body = Limited::new(response.into_body(), max_bytes)
        .collect()
        .await
        .map_err(|error| anyhow!("{error}"))?;
    Ok(body.to_bytes().to_vec())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use blake2::Blake2b512;
    use ed25519_dalek::{Signer, SigningKey};

    const KEY_ID: [u8; 8] = *b"testkey1";

    /// A minisign key pair in Tauri's base64 format, for tests.
    pub(crate) struct TestKey(SigningKey);

    impl TestKey {
        pub(crate) fn new(seed: u8) -> Self {
            Self(SigningKey::from_bytes(&[seed; 32]))
        }

        pub(crate) fn public(&self) -> String {
            let mut bin = b"Ed".to_vec();
            bin.extend_from_slice(&KEY_ID);
            bin.extend_from_slice(self.0.verifying_key().as_bytes());
            let file = format!("untrusted comment: minisign public key\n{}\n", b64(&bin));
            b64(file.as_bytes())
        }

        /// A prehashed signature, as `tauri signer sign` makes.
        pub(crate) fn sign(&self, data: &[u8]) -> String {
            let hash = Blake2b512::digest(data);
            let signature = self.0.sign(&hash).to_bytes();
            let mut bin = b"ED".to_vec();
            bin.extend_from_slice(&KEY_ID);
            bin.extend_from_slice(&signature);
            let trusted = "timestamp:1\tfile:Localify.app.tar.gz";
            let mut global = signature.to_vec();
            global.extend_from_slice(trusted.as_bytes());
            let global = self.0.sign(&global).to_bytes();
            let file = format!(
                "untrusted comment: signature from tauri secret key\n{}\ntrusted comment: {trusted}\n{}\n",
                b64(&bin),
                b64(&global)
            );
            b64(file.as_bytes())
        }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn a_signature_from_the_key_verifies() {
        let key = TestKey::new(7);
        let data = b"the archive";
        verify_signature(data, &key.sign(data), &key.public()).unwrap();
    }

    #[test]
    fn a_changed_archive_or_another_key_fails() {
        let key = TestKey::new(7);
        let signature = key.sign(b"the archive");
        assert!(verify_signature(b"the archivf", &signature, &key.public()).is_err());
        let other = TestKey::new(8);
        assert!(verify_signature(b"the archive", &signature, &other.public()).is_err());
        assert!(verify_signature(b"the archive", "not base64!", &key.public()).is_err());
    }

    #[test]
    fn the_built_in_key_is_readable() {
        let text = decode_text(updater_public_key()).unwrap();
        PublicKey::decode(&text).unwrap();
    }

    #[test]
    fn the_endpoint_comes_from_the_endpoint_or_the_base_url() {
        assert_eq!(
            endpoint_from(Some(" https://a.test/m.json "), Some("https://b.test")),
            Some("https://a.test/m.json".into())
        );
        assert_eq!(
            endpoint_from(None, Some("https://b.test/")),
            Some("https://b.test/latest.json".into())
        );
        assert_eq!(endpoint_from(Some(""), Some(" ")), None);
        assert_eq!(endpoint_from(None, None), None);
    }

    const MANIFEST: &str = r#"{
        "version": "1.7.0",
        "notes": "Localify 1.7.0",
        "platforms": {
            "darwin-aarch64": {"signature": "c2ln", "url": "https://d.test/1.7.0/a.tar.gz"}
        }
    }"#;

    #[test]
    fn the_manifest_names_the_platforms_archive() {
        let release = parse_manifest(MANIFEST.as_bytes(), Some("darwin-aarch64")).unwrap();
        assert_eq!(release.version, "1.7.0");
        let archive = release.archive.unwrap();
        assert_eq!(archive.url, "https://d.test/1.7.0/a.tar.gz");
        assert_eq!(archive.signature, "c2ln");
    }

    #[test]
    fn a_release_is_known_by_its_version_and_signature() {
        let release = parse_manifest(MANIFEST.as_bytes(), Some("darwin-aarch64")).unwrap();
        let mut resigned = release.clone();
        resigned.archive.as_mut().unwrap().signature = "b3RoZXI=".into();
        let mut renamed = release.clone();
        renamed.version = "1.7.1".into();
        assert_eq!(release.identity(), release.clone().identity());
        assert_ne!(release.identity(), resigned.identity());
        assert_ne!(release.identity(), renamed.identity());
        assert_eq!(release.identity().len(), 64);
    }

    #[test]
    fn without_a_platform_only_the_version_is_read() {
        let release = parse_manifest(MANIFEST.as_bytes(), None).unwrap();
        assert_eq!(release.version, "1.7.0");
        assert_eq!(release.archive, None);
    }

    #[test]
    fn a_manifest_missing_the_platform_or_over_http_is_refused() {
        assert!(parse_manifest(MANIFEST.as_bytes(), Some("darwin-x86_64")).is_err());
        let http = MANIFEST.replace("https://", "http://");
        assert!(parse_manifest(http.as_bytes(), Some("darwin-aarch64")).is_err());
        let bad = MANIFEST.replace(r#""version": "1.7.0""#, r#""version": "seven""#);
        assert!(parse_manifest(bad.as_bytes(), None).is_err());
    }

    #[tokio::test]
    async fn a_fetch_runs_on_the_daemons_blocking_threads() {
        // Its own runtime starts there without a nested-runtime panic; the
        // fetch itself fails, since nothing listens.
        let result = tokio::task::spawn_blocking(|| {
            get(
                "https://127.0.0.1:9/latest.json",
                10,
                Duration::from_secs(5),
            )
        })
        .await;
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn only_https_urls_are_fetched() {
        let error = get("http://example.test/latest.json", 10, MANIFEST_TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("HTTPS"));
    }
}
