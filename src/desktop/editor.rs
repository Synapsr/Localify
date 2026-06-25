use std::path::Path;

use anyhow::Result;

pub fn open_in_editor(path: &Path) -> Result<()> {
    crate::infrastructure::editor::open_in_editor(path)
}

/// Opens a file target in the configured editor without creating it first.
pub fn open_file_in_editor(path: &Path) -> Result<()> {
    crate::infrastructure::editor::open_file_in_editor(path)
}
