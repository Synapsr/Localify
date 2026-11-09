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

use super::dns_server::DnsServer;
use super::launch_sessions::LaunchSessions;
use super::lifecycle::ShutdownHandle;
use super::listeners::DaemonListeners;
use super::mgmt_socket::MachineServices;
use super::proxy::{ClientAddr, Scheme};
use super::public_ingress::{self, IngressAccess};
use super::public_tunnel::{self, PublicTunnel, TunnelFiles};
use super::remote::local::{AppSeen, DaemonSource};
use super::remote::{self, RemoteLink};
use super::router::{DomainAccessLog, RuntimeState, SharedState, create_router};
use super::tls::create_tls_acceptor;
use crate::application::ports::RegistrationProvider;
use crate::branding::BRANDING;
use crate::infrastructure::config::Config;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::inventory::LocalInventory;
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

pub struct Server {
    state: SharedState,
    reload_rx: mpsc::Receiver<()>,
    reload_tx: mpsc::Sender<()>,
    reload_provider: Arc<dyn RegistrationProvider>,
    config_path: PathBuf,
    socket_path: PathBuf,
    data_dir: PathBuf,
    log_file: PathBuf,
    tls_acceptor: Option<TlsAcceptor>,
    listeners: DaemonListeners,
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
        reload_rx: mpsc::Receiver<()>,
        reload_tx: mpsc::Sender<()>,
        reload_provider: Arc<dyn RegistrationProvider>,
        listeners: DaemonListeners,
    ) -> Result<Self> {
        // Validate config before starting
        config.validate()?;

        let registrations = reload_provider
            .load()
            .context("Failed to load registrations for daemon startup")?;
        let projects = ProjectMetadata::of(config);

        // Collect patterns for domains with HTTPS enabled
        let https_patterns: Vec<_> = registrations
            .iter()
            .filter(|d| d.is_https_enabled())
            .map(|d| d.pattern().clone())
            .collect();

        let domain_accesses = DomainAccessLog::default();
        let state: SharedState = Arc::new(ArcSwap::from_pointee(
            RuntimeState::with_project_roots_and_access_log(
                registrations,
                projects.roots,
                domain_accesses.clone(),
            )
            .with_domain_projects(projects.domains)
            .with_exposable_domains(projects.exposable)
            .with_launch_sessions(LaunchSessions::default()),
        ));

        let tls_acceptor = create_tls_acceptor(&https_patterns, &paths.certs_dir, &paths.data_dir)?;

        // Get LAN IP for DNS responses (DNS server handles source-based resolution)
        let lan_ip = get_lan_ip();

        Ok(Self {
            state,
            reload_rx,
            reload_tx,
            reload_provider,
            config_path,
            socket_path: paths.socket_path.clone(),
            data_dir: paths.data_dir.clone(),
            log_file: paths.log_file.clone(),
            tls_acceptor,
            listeners,
            http_port: config.daemon.http_port,
            https_port: config.daemon.https_port,
            dns_port: config.daemon.dns_port,
            lan_ip,
        })
    }

    pub async fn run(self, shutdown: ShutdownHandle) -> Result<()> {
        let cancel = shutdown.token();
        info!(
            http = self.http_port,
            https = self.https_port,
            dns = self.dns_port,
            lan_ip = %self.lan_ip,
            lan_access = self.listeners.lan_access,
            "Localify daemon starting"
        );

        let dns_handle = spawn_dns_server(self.dns_port, self.listeners.lan_access);
        let domain_accesses = self.state.load().domain_access_log();
        let launch_sessions = self.state.load().launch_sessions();
        let inventory = Arc::new(LocalInventory::new(
            self.config_path.clone(),
            self.data_dir.clone(),
        )?);
        spawn_reload_listener(
            self.state.clone(),
            self.reload_rx,
            self.reload_provider,
            self.config_path,
            domain_accesses,
            launch_sessions,
            cancel.clone(),
        );
        let tunnel = PublicTunnel::new(BRANDING.public_ingress_port());
        let access = IngressAccess::default();
        spawn_public_ingress(
            self.state.clone(),
            access.clone(),
            tunnel.clone(),
            cancel.clone(),
        )
        .await;
        let app = AppSeen::watch(&self.data_dir, cancel.clone());
        let source = DaemonSource::new(Arc::clone(&inventory), tunnel.clone(), app);
        let remote = RemoteLink::new(&self.data_dir, Arc::new(source));
        remote::spawn(remote.clone(), cancel.clone());
        public_tunnel::spawn(
            tunnel.clone(),
            access,
            TunnelFiles {
                data_dir: self.data_dir,
                daemon_log: self.log_file,
            },
            cancel.clone(),
        );
        let ports = super::mgmt_socket::DaemonPorts {
            http: self.http_port,
            https: self.https_port,
            dns: self.dns_port,
        };
        spawn_mgmt_socket(
            self.state.clone(),
            self.reload_tx,
            self.socket_path,
            ports,
            shutdown,
            MachineServices {
                tunnel,
                inventory,
                remote,
            },
        );

        let http_server =
            start_http_server(self.state.clone(), self.listeners.http, cancel.clone())?;

        await_servers(
            http_server,
            dns_handle,
            self.state,
            self.tls_acceptor,
            self.listeners.https,
            cancel,
        )
        .await
    }
}

fn spawn_dns_server(port: u16, lan_access: bool) -> tokio::task::JoinHandle<Result<()>> {
    let dns_server = DnsServer::new(port, lan_access);
    tokio::spawn(async move { dns_server.run().await })
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
                                let projects = ProjectMetadata::from_store(Path::new(&config_path));
                                let new = Arc::new(RuntimeState::with_project_roots_and_access_log(
                                    regs,
                                    projects.roots,
                                    domain_accesses.clone(),
                                )
                                .with_domain_projects(projects.domains)
                                .with_exposable_domains(projects.exposable)
                                .with_launch_sessions(launch_sessions.clone()));
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

/// What the runtime state needs to know about projects, by domain pattern.
#[derive(Default)]
struct ProjectMetadata {
    /// Project folder of each project domain, for the editor shortcuts.
    roots: HashMap<String, String>,
    /// Project id of each domain of an enabled project, for public traffic.
    domains: HashMap<String, String>,
    /// Project id of each exposable local domain, for public traffic.
    exposable: HashMap<String, String>,
}

impl ProjectMetadata {
    fn of(config: &Config) -> Self {
        Self {
            roots: load_project_root_paths(config),
            domains: config
                .configured_domains()
                .into_iter()
                .filter(|domain| domain.project_enabled)
                .filter_map(|domain| {
                    Some((domain.registration.display_pattern(), domain.project_id?))
                })
                .collect(),
            exposable: config.exposable_domains(),
        }
    }

    fn from_store(config_path: &Path) -> Self {
        match ConfigStore::new(config_path.to_path_buf()).load() {
            Ok(config) => Self::of(&config),
            Err(error) => {
                warn!(
                    error = %error,
                    config_path = %config_path.display(),
                    "Failed to load project metadata during daemon reload"
                );
                Self::default()
            }
        }
    }
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

fn load_local_project_root_paths() -> HashMap<String, String> {
    ProjectRootStore::new_default()
        .ok()
        .and_then(|store| store.load().ok())
        .unwrap_or_default()
}

/// The listener binds whether or not the machine is signed in: until the
/// tunnel supervisor grants access, it answers every request with 403. A
/// port held by another program does not stop the daemon; the public status
/// says why public URLs cannot work.
async fn spawn_public_ingress(
    state: SharedState,
    access: IngressAccess,
    tunnel: PublicTunnel,
    cancel: CancellationToken,
) {
    let port = BRANDING.public_ingress_port();
    match public_ingress::bind(port).await {
        Ok(listener) => {
            let router = public_ingress::ingress_router(state, access);
            tokio::spawn(public_ingress::serve(listener, router, cancel));
        }
        Err(error) => {
            warn!(port, error = %error, "Public ingress could not listen");
            tunnel.ingress_failed(format!(
                "{} could not listen on 127.0.0.1:{port} for public traffic ({error}). Quit the program that uses this port, then restart {}.",
                BRANDING.app_name, BRANDING.app_name
            ));
        }
    }
}

fn spawn_mgmt_socket(
    state: SharedState,
    reload_tx: mpsc::Sender<()>,
    socket_path: PathBuf,
    ports: super::mgmt_socket::DaemonPorts,
    shutdown: ShutdownHandle,
    services: MachineServices,
) {
    tokio::spawn(async move {
        let served =
            super::mgmt_socket::serve(socket_path, state, reload_tx, shutdown, ports, services);
        if let Err(e) = served.await {
            error!(error = %e, "Management socket error");
        }
    });
}

fn start_http_server(
    state: SharedState,
    listeners: Vec<std::net::TcpListener>,
    cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<Result<()>>> {
    let mut servers = tokio::task::JoinSet::new();

    for listener in listeners {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        info!(addr = %listener.local_addr()?, "HTTP server listening");

        let router = create_router(state.clone())
            .layer(Extension(Scheme::Http))
            .layer(axum::middleware::from_fn(inject_client_addr));
        let cancel = cancel.clone();
        servers.spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .map_err(|e| anyhow::anyhow!("HTTP server error: {}", e))
        });
    }

    Ok(tokio::spawn(async move {
        while let Some(result) = servers.join_next().await {
            result??;
        }
        Ok(())
    }))
}

fn start_https_server(
    state: SharedState,
    tls_acceptor: TlsAcceptor,
    listeners: Vec<std::net::TcpListener>,
    cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>> {
    let mut servers = tokio::task::JoinSet::new();

    for listener in listeners {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        info!(addr = %listener.local_addr()?, "HTTPS server listening");
        servers.spawn(serve_https(
            state.clone(),
            tls_acceptor.clone(),
            listener,
            cancel.clone(),
        ));
    }

    Ok(tokio::spawn(async move {
        while servers.join_next().await.is_some() {}
    }))
}

async fn serve_https(
    state: SharedState,
    tls_acceptor: TlsAcceptor,
    listener: TcpListener,
    cancel: CancellationToken,
) {
    let https_router = create_router(state).layer(Extension(Scheme::Https));

    loop {
        let (stream, addr) = tokio::select! {
            _ = cancel.cancelled() => break,
            result = listener.accept() => match result {
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

            if let Err(e) =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(io, service)
                    .await
            {
                error!(error = %e, "Error serving connection");
            }
        });
    }
}

async fn await_servers(
    http_server: tokio::task::JoinHandle<Result<()>>,
    dns_handle: tokio::task::JoinHandle<Result<()>>,
    state: SharedState,
    tls_acceptor: Option<TlsAcceptor>,
    https_listeners: Vec<std::net::TcpListener>,
    cancel: CancellationToken,
) -> Result<()> {
    if let Some(tls_acceptor) = tls_acceptor {
        let https_server =
            start_https_server(state, tls_acceptor, https_listeners, cancel.clone())?;

        tokio::select! {
            r = http_server => r??,
            r = https_server => {
                if let Err(e) = r {
                    error!(error = %e, "HTTPS server task failed");
                    anyhow::bail!("HTTPS server task failed: {e}");
                }
            },
            r = dns_handle => {
                r.context("DNS server task failed")?.context("DNS server failed")?;
            },
        }
    } else {
        warn!(
            "No HTTPS certificates found, running HTTP only. \
             Run the install step to enable HTTPS."
        );
        tokio::select! {
            r = http_server => r??,
            r = dns_handle => {
                r.context("DNS server task failed")?.context("DNS server failed")?;
            },
        }
    }

    Ok(())
}
