use std::process::{Command, Stdio};

use super::ProcessControl;

pub struct UnixProcessControl;

impl ProcessControl for UnixProcessControl {
    fn process_exists(&self, pid: u32) -> bool {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|output| {
                output.status.success()
                    || String::from_utf8_lossy(&output.stderr).contains("Operation not permitted")
            })
            .unwrap_or(false)
    }
}

/// Signals a process and every process it started (`pnpm` → `next` → workers).
pub fn signal_process_tree(root_pid: u32, signal: &str) {
    let mut pids = vec![root_pid];
    let mut index = 0;
    while index < pids.len() {
        pids.extend(child_pids(pids[index]));
        index += 1;
    }
    let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
    let _ = Command::new("kill")
        .arg(format!("-{signal}"))
        .args(&pids)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn child_pids(pid: u32) -> Vec<u32> {
    Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// When a process started, as `ps` prints it. A PID comes back once its process
/// has ended, so a PID read from a file is only trusted with the same start time.
pub fn process_started_at(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let started = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !started.is_empty()).then_some(started)
}
