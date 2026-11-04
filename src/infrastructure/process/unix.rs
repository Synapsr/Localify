use std::process::Command;

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
