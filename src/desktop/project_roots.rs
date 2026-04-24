use std::collections::HashMap;
#[cfg(not(target_os = "macos"))]
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::branding::BRANDING;
use crate::desktop::home_dir::user_home_dir;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ProjectRootsFile {
    #[serde(default)]
    projects: HashMap<String, String>,
}

pub struct ProjectRootStore {
    path: PathBuf,
}

impl ProjectRootStore {
    pub fn new_default() -> Result<Self> {
        Ok(Self {
            path: default_project_roots_path()?,
        })
    }

    pub fn load(&self) -> Result<HashMap<String, String>> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }

        let content = fs::read_to_string(&self.path).with_context(|| {
            format!(
                "Failed to read project root metadata from '{}'",
                self.path.display()
            )
        })?;
        let file: ProjectRootsFile = serde_json::from_str(&content).with_context(|| {
            format!(
                "Failed to parse project root metadata from '{}'",
                self.path.display()
            )
        })?;
        Ok(file.projects)
    }

    pub fn set(&self, project_id: &str, root_path: &Path) -> Result<()> {
        let mut projects = self.load()?;
        projects.insert(project_id.to_string(), root_path.display().to_string());
        self.save(projects)
    }

    pub fn remove(&self, project_id: &str) -> Result<()> {
        let mut projects = self.load()?;
        if projects.remove(project_id).is_some() {
            self.save(projects)?;
        }
        Ok(())
    }

    fn save(&self, projects: HashMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create project root metadata directory '{}'",
                    parent.display()
                )
            })?;
        }

        let content = serde_json::to_string_pretty(&ProjectRootsFile { projects })
            .context("Failed to serialize project root metadata")?;
        fs::write(&self.path, format!("{content}\n")).with_context(|| {
            format!(
                "Failed to write project root metadata to '{}'",
                self.path.display()
            )
        })?;
        Ok(())
    }
}

fn default_project_roots_path() -> Result<PathBuf> {
    Ok(default_project_roots_dir()?.join("project-roots.json"))
}

fn default_project_roots_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(user_home_dir()?
            .join("Library/Application Support")
            .join(BRANDING.desktop_state_dir_name))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let base = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                user_home_dir()
                    .map(|home| home.join(".local/state"))
                    .unwrap()
            });
        Ok(base.join(BRANDING.slug))
    }
}
