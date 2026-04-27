#[cfg(not(target_os = "macos"))]
use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::desktop::home_dir::user_home_dir;
use crate::infrastructure::editor::EditorPreference;
use crate::infrastructure::local_dev::TerminalPreference;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunActionPreference {
    #[default]
    TerminalOnly,
    EditorAndTerminal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherPreferences {
    #[serde(default)]
    pub editor: EditorPreference,
    #[serde(default)]
    pub terminal: TerminalPreference,
    #[serde(default)]
    pub run_action: RunActionPreference,
}

pub struct LauncherPreferencesStore {
    path: PathBuf,
}

impl LauncherPreferencesStore {
    pub fn new_default() -> Result<Self> {
        Ok(Self {
            path: default_launcher_preferences_path()?,
        })
    }

    pub fn load(&self) -> Result<LauncherPreferences> {
        if !self.path.exists() {
            return Ok(LauncherPreferences::default());
        }

        let content = fs::read_to_string(&self.path).with_context(|| {
            format!(
                "Failed to read launcher preferences from '{}'",
                self.path.display()
            )
        })?;
        serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse launcher preferences from '{}'",
                self.path.display()
            )
        })
    }

    pub fn save(&self, preferences: &LauncherPreferences) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create launcher preferences directory '{}'",
                    parent.display()
                )
            })?;
        }

        let content = serde_json::to_string_pretty(preferences)
            .context("Failed to serialize launcher preferences")?;
        fs::write(&self.path, format!("{content}\n")).with_context(|| {
            format!(
                "Failed to write launcher preferences to '{}'",
                self.path.display()
            )
        })?;
        Ok(())
    }
}

fn default_launcher_preferences_path() -> Result<PathBuf> {
    Ok(default_local_state_dir()?.join("launcher-preferences.json"))
}

fn default_local_state_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(user_home_dir()?
            .join("Library/Application Support")
            .join(BRANDING.desktop_state_dir_name()))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let base = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| user_home_dir().unwrap().join(".local/state"));
        Ok(base.join(BRANDING.slug()))
    }
}
