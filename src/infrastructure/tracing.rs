use std::fs::{self, OpenOptions};
use std::path::PathBuf;

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::branding::BRANDING;

/// Dependencies whose debug and trace output carries secrets: tungstenite
/// logs the whole upgrade request, `Authorization: Bearer <token>`
/// included. They log at `warn` at most, whatever was asked.
const QUIET_TARGETS: &[&str] = &["tungstenite", "tokio_tungstenite"];

/// The filter for `requested` (`LOCALIFY_LOG`'s syntax), with the quiet
/// targets held at `warn`: a directive naming them is dropped, so it cannot
/// turn them up. A spec that does not parse falls back to `info`.
pub fn filter_for(requested: &str) -> EnvFilter {
    let kept: Vec<&str> = requested
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .filter(|directive| {
            !QUIET_TARGETS.iter().any(|target| {
                directive
                    .strip_prefix(target)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '=', '[']))
            })
        })
        .collect();
    let mut filter = EnvFilter::try_new(kept.join(",")).unwrap_or_else(|_| EnvFilter::new("info"));
    for target in QUIET_TARGETS {
        if let Ok(directive) = format!("{target}=warn").parse() {
            filter = filter.add_directive(directive);
        }
    }
    filter
}

/// Output destination for tracing
pub enum TracingOutput {
    /// Output to stdout (for foreground/development mode)
    Stdout,
    /// Output to a log file (for daemon mode)
    File(PathBuf),
}

/// Initialize tracing based on configuration
/// Priority: LOCALIFY_LOG env > verbose flag > the config's level. Under launchd the
/// config is the only one of these the user can reach.
pub fn init_tracing(verbose: bool, config_level: &str, output: TracingOutput) {
    let requested = std::env::var(BRANDING.env_var("LOG"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if verbose {
                "debug".to_string()
            } else {
                config_level.to_string()
            }
        });
    let filter = filter_for(&requested);

    match output {
        TracingOutput::Stdout => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(false))
                .init();
        }
        TracingOutput::File(path) => {
            // Ensure log directory exists
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            // Open file for appending; fall back to stdout if inaccessible
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => {
                    tracing_subscriber::registry()
                        .with(filter)
                        .with(
                            fmt::layer()
                                .with_target(false)
                                .with_ansi(false)
                                .with_writer(file),
                        )
                        .init();
                }
                Err(e) => {
                    tracing_subscriber::registry()
                        .with(filter)
                        .with(fmt::layer().with_target(false))
                        .init();
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "Could not open log file, falling back to stdout"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// What tungstenite logs when it sends an upgrade (its `log` records
    /// reach tracing with their module as the target), at `trace`, asked
    /// for every way: the token never reaches the log.
    #[test]
    fn the_websocket_upgrade_never_logs_the_token() {
        for requested in [
            "trace",
            "tungstenite=trace",
            "trace,tokio_tungstenite=trace",
            "tungstenite::handshake::client=trace",
        ] {
            let captured = Captured::default();
            let writer = captured.clone();
            let subscriber = tracing_subscriber::registry()
                .with(filter_for(requested))
                .with(
                    fmt::layer()
                        .with_ansi(false)
                        .with_writer(move || writer.clone()),
                );
            tracing::subscriber::with_default(subscriber, || {
                tracing::trace!(
                    target: "tungstenite::handshake::client",
                    "Client handshake request:\nGET /v1/remote/connect HTTP/1.1\r\nAuthorization: Bearer lfy_dummy_token_0123\r\n"
                );
                tracing::debug!(target: "tokio_tungstenite", "Bearer lfy_dummy_token_0123");
                tracing::warn!(target: "tungstenite::protocol", "a warning still shows");
                tracing::trace!(target: "localify", "localify traces still show");
            });
            let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
            assert!(!log.contains("lfy_dummy_token_0123"), "{requested}: {log}");
            assert!(log.contains("a warning still shows"), "{requested}: {log}");
            if requested.starts_with("trace") {
                assert!(
                    log.contains("localify traces still show"),
                    "{requested}: {log}"
                );
            }
        }
    }
}
