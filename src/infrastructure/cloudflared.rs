//! The `cloudflared` binary that carries public traffic to this machine (ADR
//! 0019): a release pinned here, downloaded from GitHub on first use and
//! checked against the SHA-256 pinned next to it before it may run.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper::header::{LOCATION, USER_AGENT};
use hyper::{Request, Response, Uri};
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use sha2::{Digest, Sha256};

/// Bump together with every hash below. Each hash is the SHA-256 of the
/// release asset as GitHub serves it.
pub const VERSION: &str = "2026.10.7";

struct Asset {
    name: &'static str,
    sha256: &'static str,
    /// macOS releases are `.tgz` archives holding a `cloudflared` binary.
    archive: bool,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
static ASSET: Option<Asset> = Some(Asset {
    name: "cloudflared-darwin-arm64.tgz",
    sha256: "3586ccf397e98e5f437e60e7c307ca6ac463e110580503af8bb8e6f9ebf831bd",
    archive: true,
});

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
static ASSET: Option<Asset> = Some(Asset {
    name: "cloudflared-darwin-amd64.tgz",
    sha256: "02f3edfd87684dd5b705607688e4eb4b323f6e777b81e7b3a85af556aa356848",
    archive: true,
});

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
static ASSET: Option<Asset> = Some(Asset {
    name: "cloudflared-linux-amd64",
    sha256: "1f99fb518b83dec2ba8d870a7cc991d82a77e6196295a3d2754ba17ab9f91872",
    archive: false,
});

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
static ASSET: Option<Asset> = Some(Asset {
    name: "cloudflared-linux-arm64",
    sha256: "3df43aedc9ff291d1735b5cb576cb97e0b157488796db2e32b28aecabb445775",
    archive: false,
});

#[cfg(not(any(
    all(
        target_os = "macos",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
    all(
        target_os = "linux",
        any(target_arch = "aarch64", target_arch = "x86_64")
    ),
)))]
static ASSET: Option<Asset> = None;

const BINARY_PREFIX: &str = "cloudflared-";
const MAX_REDIRECTS: usize = 5;
/// The assets weigh about 40 MB. Anything far larger is not the release.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// A download may be slow, but not silent.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// `<data dir>/bin/cloudflared-<version>`.
pub fn binary_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("bin")
        .join(format!("{BINARY_PREFIX}{VERSION}"))
}

/// The pinned `cloudflared`, downloaded and verified first when it is not
/// there yet. Only a binary that passed the checksum is ever renamed into
/// place, so one that exists can run.
pub async fn ensure_installed(data_dir: &Path) -> Result<PathBuf> {
    let binary = binary_path(data_dir);
    if binary.is_file() {
        return Ok(binary);
    }
    let asset = ASSET.as_ref().ok_or_else(|| {
        anyhow!(
            "Public URLs need cloudflared, which Localify supports on macOS (Apple silicon and Intel) and on Linux (x86_64 and arm64) only."
        )
    })?;
    let bin_dir = data_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("Could not create {}", bin_dir.display()))?;

    let download = bin_dir.join(format!("{}.download", asset.name));
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{VERSION}/{}",
        asset.name
    );
    if let Err(error) = download_to(&url, &download).await {
        let _ = fs::remove_file(&download);
        return Err(error.context(format!(
            "Could not download cloudflared {VERSION} from GitHub. Check the connection; Localify tries again shortly"
        )));
    }
    let expected = asset.sha256;
    let checked = download.clone();
    tokio::task::spawn_blocking(move || verify_download(&checked, expected))
        .await
        .context("The cloudflared checksum check stopped")??;

    let staged = if asset.archive {
        extract(&download, &bin_dir).await?
    } else {
        download.clone()
    };
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Could not make {} executable", staged.display()))?;
    fs::rename(&staged, &binary)
        .with_context(|| format!("Could not move cloudflared to {}", binary.display()))?;
    let _ = fs::remove_file(&download);
    remove_other_versions(&bin_dir, &binary);
    Ok(binary)
}

/// Checks a download against its pinned SHA-256, and deletes it when it
/// does not match so nothing can run it.
fn verify_download(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    if actual == expected {
        return Ok(());
    }
    let _ = fs::remove_file(path);
    bail!(
        "The cloudflared {VERSION} download does not match its pinned checksum (expected {expected}, got {actual}), so Localify deleted it. Try again later, and report it at https://github.com/Synapsr/Localify/issues if it keeps happening."
    )
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("Could not read {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("Could not read {}", path.display()))?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

type HttpsClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

fn https_client() -> Result<HttpsClient> {
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider())
        .context("TLS setup failed")?
        .https_only()
        .enable_http1()
        .enable_http2()
        .build();
    Ok(Client::builder(TokioExecutor::new()).build(connector))
}

/// GitHub answers a release asset with a redirect to its storage host.
async fn download_to(url: &str, path: &Path) -> Result<()> {
    let client = https_client()?;
    let mut uri: Uri = url.parse().context("invalid download URL")?;
    for _ in 0..=MAX_REDIRECTS {
        let response = get(&client, uri.clone()).await?;
        if response.status().is_redirection() {
            uri = redirect_target(&response)?;
            continue;
        }
        if !response.status().is_success() {
            bail!("GitHub answered {}", response.status());
        }
        return write_body(response.into_body(), path).await;
    }
    bail!("too many redirects")
}

async fn get(client: &HttpsClient, uri: Uri) -> Result<Response<Incoming>> {
    let request = Request::get(uri)
        .header(USER_AGENT, concat!("localify/", env!("CARGO_PKG_VERSION")))
        .body(Empty::new())
        .context("invalid download request")?;
    tokio::time::timeout(RESPONSE_TIMEOUT, client.request(request))
        .await
        .map_err(|_| anyhow!("no answer after {} s", RESPONSE_TIMEOUT.as_secs()))?
        .context("request failed")
}

fn redirect_target(response: &Response<Incoming>) -> Result<Uri> {
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow!("a redirect without a location"))?;
    let uri: Uri = location.parse().context("an invalid redirect")?;
    if uri.scheme_str() != Some("https") {
        bail!("a redirect away from HTTPS");
    }
    Ok(uri)
}

async fn write_body(mut body: Incoming, path: &Path) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("Could not write {}", path.display()))?;
    let mut written: u64 = 0;
    loop {
        let frame = tokio::time::timeout(STALL_TIMEOUT, body.frame())
            .await
            .map_err(|_| anyhow!("the download stalled"))?;
        let Some(frame) = frame else { break };
        let Ok(data) = frame.context("the download broke off")?.into_data() else {
            continue;
        };
        written += data.len() as u64;
        if written > MAX_DOWNLOAD_BYTES {
            bail!("the download is far larger than a cloudflared release");
        }
        file.write_all(&data)
            .with_context(|| format!("Could not write {}", path.display()))?;
    }
    file.sync_all()
        .with_context(|| format!("Could not write {}", path.display()))
}

/// Unpacks the `cloudflared` binary of a verified archive with the system's
/// `tar`, and returns where it landed.
async fn extract(archive: &Path, bin_dir: &Path) -> Result<PathBuf> {
    let target = bin_dir.join(format!("{BINARY_PREFIX}{VERSION}.extract"));
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target)
        .with_context(|| format!("Could not create {}", target.display()))?;
    let output = tokio::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&target)
        .arg("cloudflared")
        .output()
        .await
        .context("Could not run tar to unpack cloudflared")?;
    if !output.status.success() {
        bail!(
            "tar could not unpack cloudflared: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let unpacked = target.join("cloudflared");
    let is_file = fs::symlink_metadata(&unpacked).is_ok_and(|meta| meta.file_type().is_file());
    if !is_file {
        bail!("The cloudflared archive holds no cloudflared binary");
    }
    let staged = bin_dir.join(format!("{BINARY_PREFIX}{VERSION}.staged"));
    fs::rename(&unpacked, &staged)
        .with_context(|| format!("Could not move {}", unpacked.display()))?;
    let _ = fs::remove_dir_all(&target);
    Ok(staged)
}

/// A version bump leaves the previous binary behind; nothing runs it again.
fn remove_other_versions(bin_dir: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(bin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(BINARY_PREFIX));
        if is_ours && path != current && path.is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn a_download_that_matches_its_checksum_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloudflared.download");
        fs::write(&path, "hello").unwrap();

        verify_download(&path, HELLO_SHA256).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn a_download_that_does_not_match_is_deleted_with_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloudflared.download");
        fs::write(&path, "tampered").unwrap();

        let error = verify_download(&path, HELLO_SHA256)
            .unwrap_err()
            .to_string();

        assert!(!path.exists());
        assert!(
            error.contains("does not match its pinned checksum"),
            "{error}"
        );
        assert!(error.contains(HELLO_SHA256), "{error}");
    }

    #[test]
    fn every_pinned_checksum_is_a_sha256() {
        if let Some(asset) = ASSET.as_ref() {
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    #[tokio::test]
    async fn an_installed_binary_is_used_without_downloading() {
        let dir = tempfile::tempdir().unwrap();
        let binary = binary_path(dir.path());
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "#!/bin/sh\n").unwrap();

        assert_eq!(ensure_installed(dir.path()).await.unwrap(), binary);
    }

    #[test]
    fn a_new_version_removes_the_old_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join(format!("{BINARY_PREFIX}{VERSION}"));
        let old = dir.path().join("cloudflared-2025.1.0");
        let unrelated = dir.path().join("other-tool");
        for path in [&current, &old, &unrelated] {
            fs::write(path, "x").unwrap();
        }

        remove_other_versions(dir.path(), &current);

        assert!(current.exists() && unrelated.exists() && !old.exists());
    }

    #[tokio::test]
    async fn an_archive_yields_its_cloudflared_binary() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("cloudflared"), "binary").unwrap();
        let archive = dir.path().join("cloudflared.tgz");
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("cloudflared")
            .status()
            .unwrap();
        assert!(status.success());

        let staged = extract(&archive, dir.path()).await.unwrap();

        assert_eq!(fs::read_to_string(staged).unwrap(), "binary");
    }
}
