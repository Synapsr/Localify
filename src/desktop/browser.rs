use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::desktop::BrowserPreference;

pub fn open_project_url(url: &str, browser: BrowserPreference) -> Result<()> {
    validate_web_url(url)?;
    let mut command = browser_open_command(url, browser)?;
    let status = command
        .status()
        .with_context(|| format!("Failed to open '{url}'."))?;

    if !status.success() {
        bail!("The selected browser could not open '{url}'.");
    }

    Ok(())
}

fn validate_web_url(url: &str) -> Result<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(());
    }

    bail!("Localify can only open http:// or https:// URLs.");
}

fn browser_open_command(url: &str, browser: BrowserPreference) -> Result<Command> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        if let Some(app_name) = macos_browser_app_name(browser) {
            command.arg("-a").arg(app_name);
        }
        command.arg(url);
        Ok(command)
    }

    #[cfg(target_os = "linux")]
    {
        if browser != BrowserPreference::Default {
            bail!("Selecting a specific browser is only supported on macOS for now.");
        }

        let mut command = Command::new("xdg-open");
        command.arg(url);
        Ok(command)
    }

    #[cfg(target_os = "windows")]
    {
        if browser != BrowserPreference::Default {
            bail!("Selecting a specific browser is only supported on macOS for now.");
        }

        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        Ok(command)
    }
}

#[cfg(target_os = "macos")]
fn macos_browser_app_name(browser: BrowserPreference) -> Option<&'static str> {
    match browser {
        BrowserPreference::Default => None,
        BrowserPreference::Helium => Some("Helium"),
        BrowserPreference::Arc => Some("Arc"),
        BrowserPreference::Chrome => Some("Google Chrome"),
        BrowserPreference::Safari => Some("Safari"),
        BrowserPreference::Firefox => Some("Firefox"),
        BrowserPreference::Brave => Some("Brave Browser"),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_web_url;

    #[test]
    fn validate_web_url_accepts_http_and_https() {
        assert!(validate_web_url("http://app.localify").is_ok());
        assert!(validate_web_url("https://app.localify").is_ok());
    }

    #[test]
    fn validate_web_url_rejects_non_web_urls() {
        assert!(validate_web_url("file:///tmp/example").is_err());
        assert!(validate_web_url("localify://settings").is_err());
    }
}
