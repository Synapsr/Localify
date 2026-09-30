use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use localify::desktop::dto::LogsRequest;
use localify::system_helper::{SystemHelperClient, current_uid, helper_socket_path};

use crate::branding::BRANDING;
use crate::infrastructure::paths::LocalifyPaths;

/// Recent lines fetched on each poll when following logs through the helper.
const HELPER_FOLLOW_WINDOW: usize = 500;

pub fn execute(lines: usize, clear: bool, follow: bool, paths: &LocalifyPaths) -> Result<()> {
    let log_path = &paths.log_file;
    if is_root_only(log_path, clear) {
        return execute_through_helper(lines, clear, follow, log_path);
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
        println!("\nStart the daemon to generate logs: sudo localify start");
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

/// The installed daemon logs into a root-only folder. Reading it as the user
/// fails with a permission error, which `Path::exists` reports as "missing".
fn is_root_only(log_path: &Path, clear: bool) -> bool {
    let access = if clear {
        OpenOptions::new().write(true).open(log_path)
    } else {
        File::open(log_path)
    };
    matches!(access, Err(error) if error.kind() == ErrorKind::PermissionDenied)
}

/// Reads or clears the root-only log through the desktop app's system helper,
/// like the desktop Logs view does.
fn execute_through_helper(lines: usize, clear: bool, follow: bool, log_path: &Path) -> Result<()> {
    let helper = current_uid()
        .ok()
        .map(|uid| SystemHelperClient::new(helper_socket_path(uid)))
        .filter(|helper| helper.ping().is_ok())
        .ok_or_else(|| {
            anyhow!(
                "{} is readable only by root.\n\
                 Run 'sudo {} logs', or open the {} desktop app once so its system helper can read it.",
                log_path.display(),
                BRANDING.cli_command(),
                BRANDING.app_name
            )
        })?;

    let logs = helper.read_logs(LogsRequest { lines, clear })?;
    if clear {
        println!("Logs cleared.");
        return Ok(());
    }
    if !logs.exists {
        println!("No logs found.");
        println!("Log file: {}", logs.path);
        return Ok(());
    }
    print!("{}", logs.content);
    if follow {
        follow_through_helper(&helper, &logs.content)?;
    }
    Ok(())
}

fn follow_through_helper(helper: &SystemHelperClient, printed: &str) -> Result<()> {
    let mut last_line = printed.lines().last().map(ToOwned::to_owned);
    loop {
        thread::sleep(Duration::from_secs(1));
        let logs = helper.read_logs(LogsRequest {
            lines: HELPER_FOLLOW_WINDOW,
            clear: false,
        })?;
        let new_lines = lines_after(&logs.content, last_line.as_deref());
        for line in &new_lines {
            println!("{line}");
        }
        if let Some(line) = new_lines.last() {
            last_line = Some(line.clone());
        }
    }
}

/// Lines after `last_seen` in a window of recent lines. Log lines carry a
/// timestamp, so the last printed line identifies the position. When it left
/// the window (log cleared, burst of lines), the whole window is new.
fn lines_after(content: &str, last_seen: Option<&str>) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = last_seen
        .and_then(|last| lines.iter().rposition(|line| *line == last))
        .map_or(0, |index| index + 1);
    lines[start..].iter().map(|line| line.to_string()).collect()
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

    #[test]
    fn follow_prints_only_lines_after_the_last_one_seen() {
        let window = "t1 a\nt2 b\nt3 c\n";

        assert_eq!(lines_after(window, Some("t2 b")), vec!["t3 c".to_string()]);
        assert!(lines_after(window, Some("t3 c")).is_empty());
        assert_eq!(lines_after(window, Some("t0 gone")).len(), 3);
        assert_eq!(lines_after(window, None).len(), 3);
    }
}
