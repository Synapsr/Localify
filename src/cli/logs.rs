use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(lines: usize, clear: bool, follow: bool, paths: &LocalifyPaths) -> Result<()> {
    let log_path = &paths.log_file;
    if is_root_only(log_path, clear) {
        bail!(
            "{} is readable only by root.\nRun: {}",
            log_path.display(),
            BRANDING.admin_command("logs")
        );
    }

    if clear {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to prepare log directory")?;
        }
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path)
            .context("Failed to clear log file")?;
        println!("Logs cleared.");
        return Ok(());
    }

    if !log_path.exists() {
        println!("No logs found.");
        println!("Log file: {}", log_path.display());
        println!(
            "\nStart the daemon to generate logs: {}",
            BRANDING.admin_command("start")
        );
        return Ok(());
    }

    // Show last N lines
    let content = tail_lines(log_path, lines)?;
    if !content.is_empty() {
        print!("{}", content);
    }

    // Follow mode: keep watching for new lines
    if follow {
        tail_follow(log_path)?;
    }

    Ok(())
}

/// A daemon on Linux logs into a root-only folder. Reading it as the user fails
/// with a permission error, which `Path::exists` reports as "missing".
fn is_root_only(log_path: &Path, clear: bool) -> bool {
    let access = if clear {
        OpenOptions::new().write(true).open(log_path)
    } else {
        File::open(log_path)
    };
    matches!(access, Err(error) if error.kind() == ErrorKind::PermissionDenied)
}

/// Read last N lines from a file
fn tail_lines(path: &std::path::Path, n: usize) -> Result<String> {
    let content = fs::read_to_string(path).context("Failed to read log file")?;
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(n);
    let result = all_lines[start..].join("\n");
    if result.is_empty() {
        Ok(result)
    } else {
        Ok(result + "\n")
    }
}

/// Follow log file for new content (like tail -f)
fn tail_follow(path: &std::path::Path) -> Result<()> {
    let file = File::open(path).context("Failed to open log file")?;
    let mut reader = BufReader::new(file);

    // Seek to end of file
    reader.seek(SeekFrom::End(0))?;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // No new data, sleep and try again
                thread::sleep(Duration::from_millis(100));
            }
            Ok(_) => {
                // New line available
                print!("{}", line);
            }
            Err(e) => {
                return Err(e).context("Error reading log file");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn root_only_logs_are_detected_instead_of_reported_missing() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let log_path = locked.join("localify.log");
        fs::write(&log_path, "line\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let root_only = is_root_only(&log_path, false);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(root_only);
        assert!(!is_root_only(&log_path, false));
        assert!(!is_root_only(&dir.path().join("missing.log"), false));
    }
}
