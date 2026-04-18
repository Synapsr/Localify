use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, Copy)]
enum EditorApp {
    Cursor,
    VsCode,
}

impl EditorApp {
    fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code",
        }
    }

    fn command_name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::VsCode => "code",
        }
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

pub fn open_in_editor(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("Project folder '{}' does not exist.", path.display());
    }

    let launch = detect_editor()
        .ok_or_else(|| anyhow!("Install Cursor or VS Code to open this project from Localify."))?;

    match launch {
        EditorLaunch::Command { program, app } => {
            let status = Command::new(&program)
                .arg(path)
                .status()
                .with_context(|| format!("Failed to launch {}.", app.label()))?;

            if !status.success() {
                bail!("{} exited before opening the project.", app.label());
            }
        }
        #[cfg(target_os = "macos")]
        EditorLaunch::OpenApp { app_name, app } => {
            let status = Command::new("open")
                .arg("-a")
                .arg(app_name)
                .arg(path)
                .status()
                .with_context(|| format!("Failed to launch {}.", app.label()))?;

            if !status.success() {
                bail!("{} exited before opening the project.", app.label());
            }
        }
    }

    Ok(())
}

fn detect_editor() -> Option<EditorLaunch> {
    detect_editor_command(EditorApp::Cursor)
        .or_else(|| detect_editor_command(EditorApp::VsCode))
        .or_else(|| detect_editor_app(EditorApp::Cursor))
        .or_else(|| detect_editor_app(EditorApp::VsCode))
}

fn detect_editor_command(app: EditorApp) -> Option<EditorLaunch> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(app.command_name()))
            .find(|candidate| command_works(candidate))
            .map(|program| EditorLaunch::Command { program, app })
    })
}

fn command_works(program: &Path) -> bool {
    program.is_file()
}

#[cfg(target_os = "macos")]
fn detect_editor_app(app: EditorApp) -> Option<EditorLaunch> {
    let app_name = match app {
        EditorApp::Cursor => "Cursor",
        EditorApp::VsCode => "Visual Studio Code",
    };
    let app_path = PathBuf::from("/Applications").join(format!("{app_name}.app"));

    if app_path.exists() {
        return Some(EditorLaunch::OpenApp { app_name, app });
    }

    None
}

#[cfg(not(target_os = "macos"))]
fn detect_editor_app(_app: EditorApp) -> Option<EditorLaunch> {
    None
}
