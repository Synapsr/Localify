import { invoke } from "@tauri-apps/api/core";

import type {
  AddRouteRequest,
  AppConfigDto,
  BootstrapDto,
  ConfigUpdateRequest,
  DaemonLaunchDto,
  DaemonStatusDto,
  DomainInfoDto,
  DomainPatternRequest,
  InstallResultDto,
  LogsDto,
  LogsRequest,
  RegisterDomainRequest,
  RegisterDomainResultDto,
  RegistrationDto,
  RegistrationMutationDto,
  RemoveRouteRequest,
  StartDaemonRequest,
  UninstallPreviewDto,
  UninstallResultDto,
  UnregisterDomainResultDto,
} from "../types/roxy";

async function tauriInvoke<T>(command: string, payload?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, payload);
}

export interface DesktopClient {
  getBootstrap(): Promise<BootstrapDto>;
  getConfig(): Promise<AppConfigDto>;
  updateConfig(request: ConfigUpdateRequest): Promise<AppConfigDto>;
  getDaemonStatus(): Promise<DaemonStatusDto>;
  listDomains(): Promise<DomainInfoDto[]>;
  installRoxy(): Promise<InstallResultDto>;
  getUninstallPreview(): Promise<UninstallPreviewDto>;
  uninstallRoxy(): Promise<UninstallResultDto>;
  registerDomain(request: RegisterDomainRequest): Promise<RegisterDomainResultDto>;
  unregisterDomain(request: DomainPatternRequest): Promise<UnregisterDomainResultDto>;
  addRoute(request: AddRouteRequest): Promise<RegistrationMutationDto>;
  removeRoute(request: RemoveRouteRequest): Promise<RegistrationMutationDto>;
  listRoutes(request: DomainPatternRequest): Promise<RegistrationDto>;
  startDaemon(request: StartDaemonRequest): Promise<DaemonLaunchDto>;
  stopDaemon(): Promise<DaemonStatusDto>;
  restartDaemon(request: StartDaemonRequest): Promise<DaemonLaunchDto>;
  reloadDaemon(request: StartDaemonRequest): Promise<DaemonLaunchDto>;
  readLogs(request: LogsRequest): Promise<LogsDto>;
}

export function createDesktopClient(): DesktopClient {
  return {
    getBootstrap: () => tauriInvoke("get_bootstrap"),
    getConfig: () => tauriInvoke("get_config"),
    updateConfig: (request) => tauriInvoke("update_config", { request }),
    getDaemonStatus: () => tauriInvoke("get_daemon_status"),
    listDomains: () => tauriInvoke("list_domains"),
    installRoxy: () => tauriInvoke("install_roxy"),
    getUninstallPreview: () => tauriInvoke("get_uninstall_preview"),
    uninstallRoxy: () => tauriInvoke("uninstall_roxy"),
    registerDomain: (request) => tauriInvoke("register_domain", { request }),
    unregisterDomain: (request) => tauriInvoke("unregister_domain", { request }),
    addRoute: (request) => tauriInvoke("add_route", { request }),
    removeRoute: (request) => tauriInvoke("remove_route", { request }),
    listRoutes: (request) => tauriInvoke("list_routes", { request }),
    startDaemon: (request) => tauriInvoke("start_daemon", { request }),
    stopDaemon: () => tauriInvoke("stop_daemon"),
    restartDaemon: (request) => tauriInvoke("restart_daemon", { request }),
    reloadDaemon: (request) => tauriInvoke("reload_daemon", { request }),
    readLogs: (request) => tauriInvoke("read_logs", { request }),
  };
}
