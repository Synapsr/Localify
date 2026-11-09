//! Runs of a service: their ids, their output stream, and how a run ends
//! (docs/remote-machines.md, "Output" and "The end of a run").
//!
//! Each run has a folder, `services/output/<runId>/`, made before its record
//! is written, or for a dispatched launch at its admission, and deleted only
//! when the next run of the service registers, the service is forgotten, or
//! a launch that never registered is completed. It holds the run's `finalize.lock`, its `end.json`
//! once finalized, and for a background run its output segments. Each
//! dispatched launch has a record in `services/launches/` (see `launches`).

pub mod finish;
pub mod launches;
pub mod output;

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

const RUN_ID_BYTES: usize = 16;
/// 16 bytes in base64url without padding.
const RUN_ID_LENGTH: usize = 22;

/// One run of a service, chosen by whoever dispatched it (`--launch-id`) or
/// by `localify dev` itself. It names the run's folder, so it is checked
/// before it reaches a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RunId(String);

impl RunId {
    /// 16 random bytes from the kernel, base64url.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0_u8; RUN_ID_BYTES];
        File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .context("Failed to read random bytes for a run id from /dev/urandom")?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Accepts exactly what [`RunId::generate`] makes: 22 base64url characters.
    pub fn parse(value: &str) -> Result<Self> {
        let valid = value.len() == RUN_ID_LENGTH
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !valid {
            bail!(
                "'{value}' is not a run id: expected {RUN_ID_LENGTH} base64url characters (16 random bytes)."
            );
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for RunId {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        Self::parse(&value)
    }
}

impl From<RunId> for String {
    fn from(id: RunId) -> Self {
        id.0
    }
}

/// `services/output/<runId>/`.
pub fn run_dir(services_dir: &Path, run_id: &RunId) -> PathBuf {
    services_dir.join("output").join(run_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_parse_and_differ() {
        let first = RunId::generate().unwrap();
        let second = RunId::generate().unwrap();

        assert_eq!(first.as_str().len(), RUN_ID_LENGTH);
        assert_eq!(RunId::parse(first.as_str()).unwrap(), first);
        assert_ne!(first, second);
    }

    #[test]
    fn ids_that_could_escape_a_folder_are_refused() {
        for value in [
            "../../../../etc/passwd",
            "a/bcdefghijklmnopqrstu",
            "short",
            "",
        ] {
            assert!(RunId::parse(value).is_err(), "{value}");
        }
        assert!(RunId::parse("..aaaaaaaaaaaaaaaaaaaa").is_err());
    }
}
