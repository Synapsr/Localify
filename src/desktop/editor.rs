use std::path::Path;

use anyhow::Result;

pub fn open_in_editor(path: &Path) -> Result<()> {
    crate::infrastructure::editor::open_in_editor(path)
}

pub fn open_in_editor_with_preference(
    path: &Path,
    preference: crate::desktop::EditorPreference,
) -> Result<()> {
    crate::infrastructure::editor::open_in_editor_with_preference(path, preference)
}
