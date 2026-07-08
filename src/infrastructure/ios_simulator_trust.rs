use std::path::Path;

#[cfg(any(target_os = "macos", test))]
use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::ffi::{OsStr, OsString};
#[cfg(target_os = "macos")]
use std::process::{Command, Output};

#[cfg(any(target_os = "macos", test))]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "macos")]
use anyhow::anyhow;
#[cfg(any(target_os = "macos", test))]
use serde::Deserialize;

use crate::application::ports::IosSimulatorTrust;

pub fn trust_booted_ios_simulators(cert_path: &Path) -> Result<IosSimulatorTrust> {
    #[cfg(target_os = "macos")]
    {
        trust_booted_ios_simulators_macos(cert_path)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = cert_path;
        Ok(IosSimulatorTrust::UnsupportedPlatform)
    }
}

#[cfg(target_os = "macos")]
fn trust_booted_ios_simulators_macos(cert_path: &Path) -> Result<IosSimulatorTrust> {
    if !cert_path.exists() {
        return Ok(IosSimulatorTrust::CertificateMissing {
            path: cert_path.to_path_buf(),
        });
    }

    let Some(runner) = SimctlRunner::for_current_user()? else {
        return Ok(IosSimulatorTrust::CurrentUserUnavailable);
    };

    let list_output = match runner.run(["simctl", "list", "devices", "booted", "--json"]) {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IosSimulatorTrust::ToolUnavailable {
                reason: format!("{} is not available.", runner.command_name()),
            });
        }
        Err(error) => return Err(error).context("Failed to run xcrun simctl"),
    };

    if !list_output.status.success() {
        let message = command_output_message(&list_output);
        if is_simctl_unavailable_message(&message) {
            return Ok(IosSimulatorTrust::ToolUnavailable { reason: message });
        }

        return Err(anyhow!(
            "Failed to list booted iOS Simulators with xcrun simctl: {}",
            message
        ));
    }

    let devices = parse_booted_ios_simulators(&String::from_utf8_lossy(&list_output.stdout))?;
    if devices.is_empty() {
        return Ok(IosSimulatorTrust::NoBootedDevices);
    }

    for device in &devices {
        let output = match runner.run_with_cert_path(
            ["simctl", "keychain", &device.udid, "add-root-cert"],
            cert_path,
        ) {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(IosSimulatorTrust::ToolUnavailable {
                    reason: format!("{} is not available.", runner.command_name()),
                });
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to run xcrun simctl for {}", device.name));
            }
        };

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to trust the Localify CA in iOS Simulator '{}': {}",
                device.name,
                command_output_message(&output)
            ));
        }
    }

    Ok(IosSimulatorTrust::Trusted {
        device_names: devices.into_iter().map(|device| device.name).collect(),
    })
}

#[cfg(target_os = "macos")]
struct SimctlRunner {
    command: OsString,
    prefix_args: Vec<OsString>,
}

#[cfg(target_os = "macos")]
impl SimctlRunner {
    fn for_current_user() -> Result<Option<Self>> {
        if current_uid()? != 0 {
            return Ok(Some(Self {
                command: "xcrun".into(),
                prefix_args: Vec::new(),
            }));
        }

        let Some(user) = std::env::var_os("SUDO_USER")
            .filter(|value| !value.is_empty() && value != OsStr::new("root"))
        else {
            return Ok(None);
        };

        if let Some(uid) = std::env::var_os("SUDO_UID").filter(|value| !value.is_empty()) {
            return Ok(Some(Self {
                command: "launchctl".into(),
                prefix_args: vec![
                    "asuser".into(),
                    uid,
                    "sudo".into(),
                    "-u".into(),
                    user,
                    "xcrun".into(),
                ],
            }));
        }

        Ok(Some(Self {
            command: "sudo".into(),
            prefix_args: vec!["-u".into(), user, "xcrun".into()],
        }))
    }

    fn command_name(&self) -> String {
        self.command.to_string_lossy().into_owned()
    }

    fn run<const N: usize>(&self, args: [&str; N]) -> std::io::Result<Output> {
        let mut command = Command::new(&self.command);
        command.args(&self.prefix_args).args(args);
        command.output()
    }

    fn run_with_cert_path<const N: usize>(
        &self,
        args: [&str; N],
        cert_path: &Path,
    ) -> std::io::Result<Output> {
        let mut command = Command::new(&self.command);
        command.args(&self.prefix_args).args(args).arg(cert_path);
        command.output()
    }
}

#[cfg(target_os = "macos")]
fn current_uid() -> Result<u32> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to determine the current user ID")?;

    if !output.status.success() {
        return Err(anyhow!(
            "Failed to determine the current user ID: {}",
            command_output_message(&output)
        ));
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .context("Failed to parse the current user ID")
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BootedIosSimulator {
    name: String,
    udid: String,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Deserialize)]
struct SimctlDeviceList {
    devices: BTreeMap<String, Vec<SimctlDevice>>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Deserialize)]
struct SimctlDevice {
    name: String,
    udid: String,
    state: String,
    #[serde(default = "default_available", rename = "isAvailable")]
    is_available: bool,
}

#[cfg(any(target_os = "macos", test))]
fn default_available() -> bool {
    true
}

#[cfg(any(target_os = "macos", test))]
fn parse_booted_ios_simulators(output: &str) -> Result<Vec<BootedIosSimulator>> {
    let parsed: SimctlDeviceList =
        serde_json::from_str(output).context("Failed to parse xcrun simctl device list")?;

    let mut devices = Vec::new();
    for (runtime, runtime_devices) in parsed.devices {
        if !runtime.starts_with("com.apple.CoreSimulator.SimRuntime.iOS") {
            continue;
        }

        devices.extend(
            runtime_devices
                .into_iter()
                .filter(|device| device.is_available && device.state == "Booted")
                .map(|device| BootedIosSimulator {
                    name: device.name,
                    udid: device.udid,
                }),
        );
    }

    Ok(devices)
}

#[cfg(target_os = "macos")]
fn command_output_message(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = format!("{} {}", stdout.trim(), stderr.trim())
        .trim()
        .to_string();

    if message.is_empty() {
        return format!(
            "command exited with status {}",
            output
                .status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string())
        );
    }

    message
}

#[cfg(target_os = "macos")]
fn is_simctl_unavailable_message(message: &str) -> bool {
    message.contains("unable to find utility")
        || message.contains("requires Xcode")
        || message.contains("xcode-select")
}

#[cfg(test)]
mod tests {
    use super::parse_booted_ios_simulators;

    #[test]
    fn parses_booted_ios_simulators_only() {
        let output = r#"
{
  "devices": {
    "com.apple.CoreSimulator.SimRuntime.iOS-26-5": [
      {
        "name": "iPhone 17",
        "udid": "A73BA2B6-58CE-4741-B284-B7E0E5A7A8CB",
        "state": "Booted",
        "isAvailable": true
      },
      {
        "name": "iPhone 17 Pro",
        "udid": "FE96459C-BF93-4C78-A7F3-675D13E730CC",
        "state": "Shutdown",
        "isAvailable": true
      }
    ],
    "com.apple.CoreSimulator.SimRuntime.watchOS-26-5": [
      {
        "name": "Apple Watch",
        "udid": "6F2812CA-873B-4355-B8F7-5B2D50299C21",
        "state": "Booted",
        "isAvailable": true
      }
    ]
  }
}
"#;

        let devices = parse_booted_ios_simulators(output).unwrap();

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "iPhone 17");
        assert_eq!(devices[0].udid, "A73BA2B6-58CE-4741-B284-B7E0E5A7A8CB");
    }

    #[test]
    fn parses_no_booted_ios_simulators() {
        let output = r#"
{
  "devices": {
    "com.apple.CoreSimulator.SimRuntime.iOS-26-5": [
      {
        "name": "iPhone 17",
        "udid": "A73BA2B6-58CE-4741-B284-B7E0E5A7A8CB",
        "state": "Shutdown",
        "isAvailable": true
      }
    ]
  }
}
"#;

        let devices = parse_booted_ios_simulators(output).unwrap();

        assert!(devices.is_empty());
    }
}
