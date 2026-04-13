import "./styles.css";

import { formatBoolean, formatNullable, titleCase } from "./lib/format";
import { AppStore, type AppState, type ScreenId } from "./state/app-store";
import type { DomainInfoDto, RegistrationDto, RouteDto } from "./types/roxy";

const root = document.querySelector<HTMLDivElement>("#app");

if (!root) {
  throw new Error("Missing #app root element");
}

const store = new AppStore();

const screens: Array<{ id: ScreenId; label: string; eyebrow: string }> = [
  { id: "overview", label: "Overview", eyebrow: "System" },
  { id: "domains", label: "Domains", eyebrow: "Routing" },
  { id: "config", label: "Config", eyebrow: "Runtime" },
  { id: "logs", label: "Logs", eyebrow: "Observability" },
  { id: "api", label: "API", eyebrow: "Frontend wiring" },
];

store.subscribe((state) => {
  root.innerHTML = renderApp(state);
});

root.addEventListener("click", async (event) => {
  const target = event.target as HTMLElement | null;
  const actionable = target?.closest<HTMLElement>("[data-action]");

  if (!actionable) {
    return;
  }

  const action = actionable.dataset.action;

  if (action === "select-screen") {
    const screen = actionable.dataset.screen as ScreenId | undefined;
    if (screen) {
      store.setScreen(screen);
      if (screen === "logs" && !store.snapshot().logs) {
        await store.refreshLogs();
      }
    }
    return;
  }

  if (action === "refresh-bootstrap") {
    await store.bootstrap();
    return;
  }

  if (action === "refresh-logs") {
    await store.refreshLogs();
    return;
  }

  if (action === "dismiss-error") {
    store.clearError();
  }
});

void store.bootstrap();

function renderApp(state: AppState): string {
  const currentScreen = screens.find((screen) => screen.id === state.screen) ?? screens[0];
  const bootstrap = state.bootstrap;

  return `
    <div class="app-shell">
      <aside class="sidebar">
        <div class="sidebar__brand">
          <span class="sidebar__kicker">Local control room</span>
          <h1>Roxy Desktop</h1>
          <p>Tauri shell ready for the real UI. Rust commands are already wired.</p>
        </div>

        <nav class="sidebar__nav" aria-label="Primary">
          ${screens
            .map(
              (screen) => `
                <button
                  class="nav-chip ${screen.id === state.screen ? "is-active" : ""}"
                  data-action="select-screen"
                  data-screen="${screen.id}"
                >
                  <span>${screen.eyebrow}</span>
                  <strong>${screen.label}</strong>
                </button>
              `,
            )
            .join("")}
        </nav>

        <section class="sidebar__summary">
          <div>
            <span>Daemon</span>
            <strong>${bootstrap?.daemon.running ? "Running" : "Stopped"}</strong>
          </div>
          <div>
            <span>Domains</span>
            <strong>${bootstrap?.domains.length ?? 0}</strong>
          </div>
          <div>
            <span>Binary</span>
            <strong>${bootstrap?.environment.binaryExists ? "Present" : "Missing"}</strong>
          </div>
        </section>
      </aside>

      <main class="content">
        <header class="hero">
          <div>
            <span class="eyebrow">${currentScreen.eyebrow}</span>
            <h2>${currentScreen.label}</h2>
            <p>
              ${renderHeroCopy(state)}
            </p>
          </div>

          <div class="hero__actions">
            <button class="button button--secondary" data-action="refresh-bootstrap">
              ${state.loading ? "Refreshing..." : "Refresh data"}
            </button>
            <button class="button button--ghost" data-action="refresh-logs">
              ${state.logsLoading ? "Loading logs..." : "Refresh logs"}
            </button>
          </div>
        </header>

        ${state.error ? renderErrorBanner(state.error) : ""}

        <section class="panel-grid panel-grid--top">
          ${renderTopCards(state)}
        </section>

        <section class="panel panel--screen">
          ${renderScreen(state)}
        </section>
      </main>
    </div>
  `;
}

function renderHeroCopy(state: AppState): string {
  if (!state.initialized) {
    return "Loading the desktop shell state from the Rust backend.";
  }

  switch (state.screen) {
    case "overview":
      return "System snapshot, launch assumptions, and the places where the future UI will read from first.";
    case "domains":
      return "Registered domains and routes from the backend facade. Editing flows can be attached directly here next.";
    case "config":
      return "Current daemon/runtime configuration, already available through typed Tauri commands.";
    case "logs":
      return "Basic log preview wired to the Rust backend. Stream/follow can be layered on later without changing the command shape.";
    case "api":
      return "Exact command surface the frontend can call. This is the bridge between the shell and the existing Roxy core.";
  }
}

function renderErrorBanner(error: string): string {
  return `
    <section class="panel panel--error">
      <div>
        <span class="eyebrow">Backend error</span>
        <strong>${escapeHtml(error)}</strong>
      </div>
      <button class="button button--ghost" data-action="dismiss-error">Dismiss</button>
    </section>
  `;
}

function renderTopCards(state: AppState): string {
  const bootstrap = state.bootstrap;

  if (!bootstrap) {
    return `
      <article class="card card--placeholder">
        <span class="eyebrow">Waiting</span>
        <strong>Bootstrap is still loading.</strong>
        <p>The store is already wired; once data resolves, these cards fill automatically.</p>
      </article>
    `;
  }

  return `
    ${renderStatCard(
      "Daemon",
      bootstrap.daemon.running ? "Running" : "Stopped",
      `PID ${formatNullable(bootstrap.daemon.pid)} · LAN ${bootstrap.daemon.lanIp}`,
    )}
    ${renderStatCard(
      "Config path",
      bootstrap.environment.configPath,
      `CLI ${bootstrap.environment.binaryExists ? "available" : "missing"} at ${bootstrap.environment.binaryPath}`,
    )}
    ${renderStatCard(
      "Certificates",
      bootstrap.daemon.caInstalled ? "Installed" : "Missing",
      `${bootstrap.domains.filter((domain) => domain.hasCert).length} domain certs present`,
    )}
  `;
}

function renderStatCard(title: string, value: string, detail: string): string {
  return `
    <article class="card stat-card">
      <span class="eyebrow">${title}</span>
      <strong>${escapeHtml(value)}</strong>
      <p>${escapeHtml(detail)}</p>
    </article>
  `;
}

function renderScreen(state: AppState): string {
  switch (state.screen) {
    case "overview":
      return renderOverview(state);
    case "domains":
      return renderDomains(state);
    case "config":
      return renderConfig(state);
    case "logs":
      return renderLogs(state);
    case "api":
      return renderApi();
  }
}

function renderOverview(state: AppState): string {
  const bootstrap = state.bootstrap;

  if (!bootstrap) {
    return renderEmpty("Bootstrapping the desktop shell.");
  }

  return `
    <div class="screen-grid">
      <article class="card">
        <span class="eyebrow">Runtime ports</span>
        <ul class="key-list">
          <li><span>HTTP</span><strong>${bootstrap.daemon.httpPort}</strong></li>
          <li><span>HTTPS</span><strong>${bootstrap.daemon.httpsPort}</strong></li>
          <li><span>DNS</span><strong>${bootstrap.daemon.dnsPort}</strong></li>
        </ul>
      </article>

      <article class="card">
        <span class="eyebrow">Filesystem anchors</span>
        <ul class="path-list">
          <li>${escapeHtml(bootstrap.config.paths.dataDir)}</li>
          <li>${escapeHtml(bootstrap.config.paths.logFile)}</li>
          <li>${escapeHtml(bootstrap.config.paths.socketPath)}</li>
        </ul>
      </article>

      <article class="card">
        <span class="eyebrow">Future app intent</span>
        <p>
          The shell already knows where config lives, how to talk to the daemon, and how to read route state.
          The next frontend pass can focus on forms and flows instead of infrastructure.
        </p>
      </article>
    </div>
  `;
}

function renderDomains(state: AppState): string {
  const domains = state.bootstrap?.domains ?? [];

  if (domains.length === 0) {
    return renderEmpty("No registered domains yet. The store and typed client are ready for create/edit flows.");
  }

  return `
    <div class="domain-stack">
      ${domains.map(renderDomainCard).join("")}
    </div>
  `;
}

function renderDomainCard(domain: DomainInfoDto): string {
  return `
    <article class="card domain-card">
      <header class="domain-card__header">
        <div>
          <span class="eyebrow">${escapeHtml(titleCase(domain.registration.source))}</span>
          <h3>${escapeHtml(domain.registration.pattern)}</h3>
        </div>
        <div class="pill-row">
          <span class="pill">${domain.registration.httpsEnabled ? "HTTPS" : "HTTP only"}</span>
          <span class="pill">${domain.hasCert ? "Cert present" : "No cert"}</span>
          <span class="pill">${domain.registration.wildcard ? "Wildcard" : "Exact"}</span>
        </div>
      </header>

      <div class="route-table">
        <div class="route-table__head">
          <span>Path</span>
          <span>Target</span>
          <span>Kind</span>
        </div>
        ${domain.registration.routes.map(renderRouteRow).join("")}
      </div>
    </article>
  `;
}

function renderRouteRow(route: RouteDto): string {
  return `
    <div class="route-table__row">
      <span>${escapeHtml(route.path)}</span>
      <span>${escapeHtml(route.target)}</span>
      <span>${escapeHtml(route.targetKind)}</span>
    </div>
  `;
}

function renderConfig(state: AppState): string {
  const config = state.bootstrap?.config;

  if (!config) {
    return renderEmpty("Config is unavailable until bootstrap finishes.");
  }

  return `
    <div class="screen-grid">
      <article class="card">
        <span class="eyebrow">Daemon settings</span>
        <ul class="key-list">
          <li><span>HTTP port</span><strong>${config.daemon.httpPort}</strong></li>
          <li><span>HTTPS port</span><strong>${config.daemon.httpsPort}</strong></li>
          <li><span>DNS port</span><strong>${config.daemon.dnsPort}</strong></li>
          <li><span>Log level</span><strong>${escapeHtml(config.daemon.logLevel)}</strong></li>
          <li><span>Docker integration</span><strong>${formatBoolean(config.docker.enabled)}</strong></li>
        </ul>
      </article>

      <article class="card">
        <span class="eyebrow">Mutable later</span>
        <p>
          The Rust backend already exposes <code>get_config</code> and <code>update_config</code>.
          The next UI iteration can add a real settings form on top of this panel without changing the command surface.
        </p>
        <pre>${escapeHtml(
          JSON.stringify(
            {
              daemon: config.daemon,
              paths: config.paths,
              docker: config.docker,
            },
            null,
            2,
          ),
        )}</pre>
      </article>
    </div>
  `;
}

function renderLogs(state: AppState): string {
  const logs = state.logs;

  if (!logs) {
    return renderEmpty("Logs are lazy-loaded. Open this tab or click refresh logs to fetch them.");
  }

  return `
    <div class="screen-grid screen-grid--wide">
      <article class="card">
        <span class="eyebrow">Log file</span>
        <ul class="key-list">
          <li><span>Path</span><strong>${escapeHtml(logs.path)}</strong></li>
          <li><span>Exists</span><strong>${formatBoolean(logs.exists)}</strong></li>
          <li><span>Cleared</span><strong>${formatBoolean(logs.cleared)}</strong></li>
        </ul>
      </article>

      <article class="card card--log">
        <span class="eyebrow">Recent entries</span>
        <pre>${escapeHtml(logs.content || "No log lines returned.")}</pre>
      </article>
    </div>
  `;
}

function renderApi(): string {
  const commands = [
    "get_bootstrap",
    "get_config",
    "update_config",
    "get_daemon_status",
    "list_domains",
    "install_roxy",
    "get_uninstall_preview",
    "uninstall_roxy",
    "register_domain",
    "unregister_domain",
    "add_route",
    "remove_route",
    "list_routes",
    "start_daemon",
    "stop_daemon",
    "restart_daemon",
    "reload_daemon",
    "read_logs",
  ];

  return `
    <div class="screen-grid">
      <article class="card">
        <span class="eyebrow">Typed frontend boundary</span>
        <p>
          Use <code>desktop-ui/src/lib/tauri-client.ts</code> from future screens. The typed request and response
          contracts already mirror the Rust DTOs.
        </p>
      </article>

      <article class="card">
        <span class="eyebrow">Current command surface</span>
        <ul class="command-list">
          ${commands.map((command) => `<li>${command}</li>`).join("")}
        </ul>
      </article>
    </div>
  `;
}

function renderEmpty(message: string): string {
  return `
    <article class="card card--placeholder">
      <span class="eyebrow">Placeholder</span>
      <strong>Screen scaffold ready.</strong>
      <p>${escapeHtml(message)}</p>
    </article>
  `;
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}
