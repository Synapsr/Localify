use std::path::Path;

use anyhow::Result;

pub fn open_in_editor(path: &Path) -> Result<()> {
    crate::infrastructure::editor::open_in_editor(path)
}
