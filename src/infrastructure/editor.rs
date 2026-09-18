use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorPreference {
    #[default]
    Auto,
    Cursor,
    VsCode,
    Windsurf,
    Zed,
    SublimeText,
    Nova,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EditorApp {
    preference: EditorPreference,
    label: &'static str,
    command_names: &'static [&'static str],
    app_names: &'static [&'static str],
}

const EDITOR_APPS: [EditorApp; 6] = [
    EditorApp {
        preference: EditorPreference::Cursor,
        label: "Cursor",
        command_names: &["cursor"],
        app_names: &["Cursor"],
    },
    EditorApp {
        preference: EditorPreference::VsCode,
        label: "VS Code",
        command_names: &["code"],
        app_names: &["Visual Studio Code"],
    },
    EditorApp {
        preference: EditorPreference::Windsurf,
        label: "Windsurf",
        command_names: &["windsurf"],
        app_names: &["Windsurf"],
    },
    EditorApp {
        preference: EditorPreference::Zed,
        label: "Zed",
        command_names: &["zed"],
        app_names: &["Zed"],
    },
    EditorApp {
        preference: EditorPreference::Nova,
        label: "Nova",
        command_names: &["nova"],
        app_names: &["Nova"],
    },
    EditorApp {
        preference: EditorPreference::SublimeText,
        label: "Sublime Text",
        command_names: &["subl"],
        app_names: &["Sublime Text"],
    },
];

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS_APP_DIRS: [&str; 3] = [
    "/Applications",
    "/System/Applications",
    "/System/Applications/Utilities",
];

impl EditorApp {
    #[allow(dead_code)]
    fn from_preference(preference: EditorPreference) -> Option<Self> {
        EDITOR_APPS
            .iter()
            .copied()
            .find(|app| app.preference == preference)
    }

    #[allow(dead_code)]
    fn available(self) -> bool {
        self.command_names
            .iter()
            .any(|command| find_command(command).is_some())
            || self
                .app_names
                .iter()
                .any(|name| find_app_bundle(name).is_some())
    }

    fn detect_launch(self) -> Option<EditorLaunch> {
        self.command_names
            .iter()
            .find_map(|command| find_command(command))
            .map(|program| EditorLaunch::Command { program, app: self })
            .or_else(|| detect_editor_app_launch(self))
    }
}

enum EditorLaunch {
    Command {
        program: PathBuf,
        app: EditorApp,
    },
    #[cfg(target_os = "macos")]
    OpenApp {
        app_name: &'static str,
        app: EditorApp,
    },
}

#[allow(dead_code)]
pub fn available_editor_preferences() -> Vec<EditorPreference> {
    EDITOR_APPS
        .iter()
        .copied()
        .filter(|app| app.available())
        .map(|app| app.preference)
        .collect()
}

#[allow(dead_code)]
pub fn editor_label(preference: EditorPreference) -> &'static str {
    match preference {
        EditorPreference::Auto => "Auto",
        _ => EditorApp::from_preference(preference)
            .map(|app| app.label)
            .unwrap_or("Unknown editor"),
    }
}

pub fn open_in_editor(path: &Path) -> Result<()> {
    open_in_editor_with_preference(path, EditorPreference::Auto)
}

pub fn open_in_editor_with_preference(path: &Path, preference: EditorPreference) -> Result<()> {
    if !path.exists() {
        bail!("Project folder '{}' does not exist.", path.display());
    }

    open_editor_target(path, preference)
}

/// Opens a file target without requiring the file to exist yet.
///
/// The parent directory must exist so Localify never creates or writes the
/// target file before handing it to the configured editor.
#[allow(dead_code)]
pub fn open_file_in_editor(path: &Path) -> Result<()> {
    if path.exists() && path.is_dir() {
        bail!("Editor target '{}' is a directory.", path.display());
    }

    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "Editor target '{}' must include a parent directory.",
            path.display()
        )
    })?;

    if !parent.is_dir() {
        bail!(
            "Editor target folder '{}' does not exist.",
            parent.display()
        );
    }

    open_editor_target(path, EditorPreference::Auto)
}

fn open_editor_target(path: &Path, preference: EditorPreference) -> Result<()> {
    let launch = detect_editor(preference).ok_or_else(|| {
        anyhow!(
            "Install a supported editor like Cursor, VS Code, Windsurf, Zed, Nova, or Sublime Text to open this project from Localify."
        )
    })?;

    match launch {
        EditorLaunch::Command { program, app } => {
            let status = Command::new(&program)
                .arg(path)
                .status()
                .with_context(|| format!("Failed to launch {}.", app.label))?;

            if !status.success() {
                bail!("{} exited before opening the project.", app.label);
            }

            #[cfg(target_os = "macos")]
            activate_editor_app(app)?;
        }
        #[cfg(target_os = "macos")]
        EditorLaunch::OpenApp { app_name, app } => {
            let status = Command::new("open")
                .arg("-a")
                .arg(app_name)
                .arg(path)
                .status()
                .with_context(|| format!("Failed to launch {}.", app.label))?;

            if !status.success() {
                bail!("{} exited before opening the project.", app.label);
            }

            activate_editor_app(app)?;
        }
    }

    Ok(())
}

fn detect_editor(preference: EditorPreference) -> Option<EditorLaunch> {
    match preference {
        EditorPreference::Auto => EDITOR_APPS.iter().find_map(|app| app.detect_launch()),
        _ => EditorApp::from_preference(preference).and_then(|app| app.detect_launch()),
    }
}

fn find_command(command_name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(command_name))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(target_os = "macos")]
fn find_app_bundle(app_name: &str) -> Option<PathBuf> {
    MACOS_APP_DIRS
        .iter()
        .map(|directory| PathBuf::from(directory).join(format!("{app_name}.app")))
        .find(|candidate| candidate.exists())
}

#[cfg(target_os = "macos")]
fn detect_editor_app_launch(app: EditorApp) -> Option<EditorLaunch> {
    app.app_names
        .iter()
        .find_map(|name| find_app_bundle(name).map(|_| *name))
        .map(|app_name| EditorLaunch::OpenApp { app_name, app })
}

#[cfg(target_os = "macos")]
fn activate_editor_app(app: EditorApp) -> Result<()> {
    let app_name = app.app_names.first().copied().ok_or_else(|| {
        anyhow!(
            "Failed to focus {} because no macOS app name is configured.",
            app.label
        )
    })?;

    let script = format!("tell application \"{}\" to activate", app_name);
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .with_context(|| format!("Failed to focus {}.", app.label))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        bail!(
            "{} opened the project, but macOS could not bring it to the front.",
            app.label
        );
    }

    bail!("{stderr}")
}

#[cfg(not(target_os = "macos"))]
fn find_app_bundle(_app_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(target_os = "macos"))]
fn detect_editor_app_launch(_app: EditorApp) -> Option<EditorLaunch> {
    None
}
