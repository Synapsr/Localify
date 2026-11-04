use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use axum::{
    Extension, extract::ConnectInfo, extract::Request, middleware::Next, response::Response,
};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use std::path::PathBuf;

use super::dns_server::{DnsServer, DnsSockets};
use super::launch_sessions::LaunchSessions;
use super::proxy::{ClientAddr, Scheme};
use super::router::{DomainAccessLog, RuntimeState, SharedState, create_router};
use super::tls::create_tls_acceptor;
use crate::application::ports::RegistrationProvider;
use crate::infrastructure::certs::OnDemandIssuer;
use crate::infrastructure::config::Config;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::network::get_lan_ip;
use crate::infrastructure::paths::LocalifyPaths;
use localify::desktop::ProjectRootStore;

/// Middleware that copies the client IP from `ConnectInfo` into a `ClientAddr` extension.
async fn inject_client_addr(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(ClientAddr(addr.ip()));
    next.run(request).await
}

/// What the daemon needs root to open. Acquired before it drops root and kept
/// open afterwards. `TlsAcceptor` holds the leaf keys in memory.
pub struct PrivilegedResources {
    http: std::net::TcpListener,
    https: Option<std::net::TcpListener>,
    dns: DnsSockets,
    mgmt: std::os::unix::net::UnixListener,
    tls_acceptor: Option<TlsAcceptor>,
}

impl PrivilegedResources {
    pub fn acquire(
        config: &Config,
        paths: &LocalifyPaths,
        issuer: Option<OnDemandIssuer>,
    ) -> Result<Self> {
        // Validate config before starting. Static routes are checked here, as
        // root, so a route to a folder the user can't read fails when requested
        // and doesn't stop every other domain.
        config.validate()?;

        let https_patterns: Vec<_> = config
            .registrations()
            .iter()
            .filter(|d| d.is_https_enabled())
            .map(|d| d.pattern().clone())
            .collect();
        let tls_acceptor = create_tls_acceptor(&https_patterns, &paths.certs_dir, issuer)?;

        let https = match &tls_acceptor {
            Some(_) => Some(bind_tcp(config.daemon.https_port)?),
            None => None,
        };

        Ok(Self {
            http: bind_tcp(config.daemon.http_port)?,
            https,
            dns: DnsSockets::bind(config.daemon.dns_port)
                .with_context(|| format!("Failed to bind DNS port {}", config.daemon.dns_port))?,
            mgmt: super::mgmt_socket::bind_socket(&paths.socket_path).with_context(|| {
                format!(
                    "Failed to bind the management socket at {}",
                    paths.socket_path.display()
                )
            })?,
            tls_acceptor,
        })
    }
}

fn bind_tcp(port: u16) -> Result<std::net::TcpListener> {
    let listener =
        std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).context(format!(
            "Failed to bind to port {port}. Is another service using it? Try: sudo lsof -i :{port}"
        ))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub struct Server {
    state: SharedState,
    reload_rx: mpsc::Receiver<()>,
    reload_tx: mpsc::Sender<()>,
    reload_provider: Arc<dyn RegistrationProvider>,
    config_path: PathBuf,
    socket_path: PathBuf,
    resources: PrivilegedResources,
    http_port: u16,
    https_port: u16,
    dns_port: u16,
    lan_ip: Ipv4Addr,
}

impl Server {
    pub fn new(
        config: &Config,
        config_path: PathBuf,
        paths: &LocalifyPaths,
        resources: PrivilegedResources,
        reload_rx: mpsc::Receiver<()>,
        reload_tx: mpsc::Sender<()>,
        reload_provider: Arc<dyn RegistrationProvider>,
    ) -> Result<Self> {
        let registrations = reload_provider
            .load()
            .context("Failed to load registrations for daemon startup")?;
        let project_roots = load_project_root_paths(config);

        let domain_accesses = DomainAccessLog::default();
        let state: SharedState = Arc::new(ArcSwap::from_pointee(
            RuntimeState::with_project_roots_and_access_log(
                registrations,
                project_roots,
                domain_accesses.clone(),
            )
            .with_launch_sessions(LaunchSessions::default()),
        ));

        // Get LAN IP for DNS responses (DNS server handles source-based resolution)
        let lan_ip = get_lan_ip();

        Ok(Self {
            state,
            reload_rx,
            reload_tx,
            reload_provider,
            config_path,
            socket_path: paths.socket_path.clone(),
            resources,
            http_port: config.daemon.http_port,
            https_port: config.daemon.https_port,
            dns_port: config.daemon.dns_port,
            lan_ip,
        })
    }

    pub async fn run(self, cancel: CancellationToken) -> Result<()> {
        info!(
            http = self.http_port,
            https = self.https_port,
            dns = self.dns_port,
            lan_ip = %self.lan_ip,
            "Localify daemon starting"
        );

        let PrivilegedResources {
            http,
            https,
            dns,
            mgmt,
            tls_acceptor,
        } = self.resources;

        let dns_handle = spawn_dns_server(dns);
        let domain_accesses = self.state.load().domain_access_log();
        let launch_sessions = self.state.load().launch_sessions();
        spawn_reload_listener(
            self.state.clone(),
            self.reload_rx,
            self.reload_provider,
            self.config_path,
            domain_accesses,
            launch_sessions,
            cancel.clone(),
        );
        spawn_mgmt_socket(
            mgmt,
            self.state.clone(),
            self.reload_tx,
            self.socket_path,
            super::mgmt_socket::DaemonPorts {
                http: self.http_port,
                https: self.https_port,
                dns: self.dns_port,
            },
            cancel.clone(),
        );

        let http_server = start_http_server(self.state.clone(), http, cancel.clone())?;
        let https = https.zip(tls_acceptor);

        await_servers(http_server, dns_handle, self.state, https, cancel).await
    }
}

fn spawn_dns_server(sockets: DnsSockets) -> tokio::task::JoinHandle<()> {
    let dns_server = DnsServer::new(sockets);
    tokio::spawn(async move {
        if let Err(e) = dns_server.run().await {
            error!(error = %e, "DNS server error");
        }
    })
}

fn spawn_reload_listener(
    state: SharedState,
    mut reload_rx: mpsc::Receiver<()>,
    reload_provider: Arc<dyn RegistrationProvider>,
    config_path: PathBuf,
    domain_accesses: DomainAccessLog,
    launch_sessions: LaunchSessions,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                msg = reload_rx.recv() => match msg {
                    Some(()) => {
                        match reload_provider.load() {
                            Ok(regs) => {
                                let count = regs.len();
                                let project_roots = load_project_root_paths_from_store(Path::new(&config_path));
                                let new = Arc::new(RuntimeState::with_project_roots_and_access_log(
                                    regs,
                                    project_roots,
                                    domain_accesses.clone(),
                                ).with_launch_sessions(launch_sessions.clone()));
                                state.store(new);
                                info!(count, "Runtime state reloaded");
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to load registrations during reload");
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    });
}

fn load_project_root_paths(config: &Config) -> HashMap<String, String> {
    let mut roots_by_project = load_local_project_root_paths();
    for project in config.configured_projects() {
        if let Some(root_path) = project.root_path {
            roots_by_project.insert(project.id, root_path);
        }
    }

    config
        .configured_domains()
        .into_iter()
        .filter_map(|domain| {
            let project_id = domain.project_id?;
            let root_path = roots_by_project.get(&project_id)?.clone();
            Some((domain.registration.display_pattern(), root_path))
        })
        .collect()
}

fn load_project_root_paths_from_store(config_path: &Path) -> HashMap<String, String> {
    match ConfigStore::new(config_path.to_path_buf()).load() {
        Ok(config) => load_project_root_paths(&config),
        Err(error) => {
            warn!(
                error = %error,
                config_path = %config_path.display(),
                "Failed to load project root metadata during daemon reload"
            );
            HashMap::new()
        }
    }
}

fn load_local_project_root_paths() -> HashMap<String, String> {
    ProjectRootStore::new_default()
        .ok()
        .and_then(|store| store.load().ok())
        .unwrap_or_default()
}

fn spawn_mgmt_socket(
    listener: std::os::unix::net::UnixListener,
    state: SharedState,
    reload_tx: mpsc::Sender<()>,
    socket_path: PathBuf,
    mgmt_ports: super::mgmt_socket::DaemonPorts,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        if let Err(e) =
            super::mgmt_socket::serve(listener, socket_path, state, reload_tx, cancel, mgmt_ports)
                .await
        {
            error!(error = %e, "Management socket error");
        }
    });
}

fn start_http_server(
    state: SharedState,
    listener: std::net::TcpListener,
    cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<Result<()>>> {
    let http_router = create_router(state)
        .layer(Extension(Scheme::Http))
        .layer(axum::middleware::from_fn(inject_client_addr));

    let http_listener = TcpListener::from_std(listener)?;

    info!(addr = %http_listener.local_addr()?, "HTTP server listening");

    Ok(tokio::spawn(async move {
        axum::serve(
            http_listener,
            http_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await
        .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))
    }))
}

fn start_https_server(
    state: SharedState,
    tls_acceptor: TlsAcceptor,
    listener: std::net::TcpListener,
    cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>> {
    let https_router = create_router(state).layer(Extension(Scheme::Https));
    let https_listener = TcpListener::from_std(listener)?;

    info!(addr = %https_listener.local_addr()?, "HTTPS server listening");

    Ok(tokio::spawn(async move {
        loop {
            let (stream, addr) = tokio::select! {
                _ = cancel.cancelled() => break,
                result = https_listener.accept() => match result {
                    Ok(conn) => conn,
                    Err(e) => {
                        error!(error = %e, "Failed to accept connection");
                        continue;
                    }
                },
            };

            let acceptor = tls_acceptor.clone();
            let router = https_router.clone().layer(Extension(ClientAddr(addr.ip())));

            tokio::spawn(async move {
                let stream = match acceptor.accept(stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, "TLS handshake failed");
                        return;
                    }
                };

                let io = hyper_util::rt::TokioIo::new(stream);
                let service = hyper_util::service::TowerToHyperService::new(router.into_service());

                if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                    hyper_util::rt::TokioExecutor::new(),
                )
                .serve_connection_with_upgrades(io, service)
                .await
                {
                    error!(error = %e, "Error serving connection");
                }
            });
        }
    }))
}

async fn await_servers(
    http_server: tokio::task::JoinHandle<Result<()>>,
    dns_handle: tokio::task::JoinHandle<()>,
    state: SharedState,
    https: Option<(std::net::TcpListener, TlsAcceptor)>,
    cancel: CancellationToken,
) -> Result<()> {
    if let Some((listener, tls_acceptor)) = https {
        let https_server = start_https_server(state, tls_acceptor, listener, cancel.clone())?;

        tokio::select! {
            r = http_server => r??,
            r = https_server => {
                if let Err(e) = r {
                    error!(error = %e, "HTTPS server task failed");
                    anyhow::bail!("HTTPS server task failed: {e}");
                }
            },
            r = dns_handle => {
                if let Err(e) = r {
                    error!(error = %e, "DNS server task failed");
                    anyhow::bail!("DNS server task failed: {e}");
                }
            },
        }
    } else {
        warn!(
            "No HTTPS certificates found, running HTTP only. \
             Register a domain with sudo to enable HTTPS."
        );
        tokio::select! {
            r = http_server => r??,
            r = dns_handle => {
                if let Err(e) = r {
                    error!(error = %e, "DNS server task failed");
                    anyhow::bail!("DNS server task failed: {e}");
                }
            },
        }
    }

    Ok(())
}
