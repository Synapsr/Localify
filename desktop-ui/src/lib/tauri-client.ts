import { invoke } from "@tauri-apps/api/core";

import { createProjectWithDomainsRequestSchema } from "./validators/validator.create-project";
import {
  detectProjectInitPlanRequestSchema,
  initProjectFromFolderRequestSchema,
} from "./validators/validator.init-project-from-folder";
import { openInEditorRequestSchema } from "./validators/validator.open-in-editor";
import { killPortConflictRequestSchema } from "./validators/validator.port-conflict";
import { removeProjectRequestSchema } from "./validators/validator.remove-project";
import type {
  AddRouteRequest,
  AssignDomainProjectRequest,
  AvailablePortDto,
  AppConfigDto,
  BootstrapDto,
  CliAccessDto,
  ConfigUpdateRequest,
  CreateProjectRequest,
  CreateProjectWithDomainsRequest,
  DaemonLaunchDto,
  DaemonStatusDto,
  DetectProjectInitPlanDto,
  DetectProjectInitPlanRequest,
  DomainInfoDto,
  DomainPatternRequest,
  InitProjectFromFolderRequest,
  InitProjectFromFolderResultDto,
  InstallResultDto,
  KillPortConflictRequest,
  LogsDto,
  LogsRequest,
  OpenInEditorRequest,
  PortConflictDto,
  ProjectInfoDto,
  RegisterDomainRequest,
  RegisterDomainResultDto,
  RegistrationDto,
  RegistrationMutationDto,
  RemoveProjectRequest,
  RemoveProjectResultDto,
  RemoveRouteRequest,
  StartDaemonRequest,
  UninstallPreviewDto,
  UninstallResultDto,
  UnregisterDomainResultDto,
  UpdateProjectRequest,
} from "../types/roxy";

async function tauriInvoke<T>(command: string, payload?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, payload);
  } catch (error) {
    throw new Error(formatTauriError(error, command));
  }
}

function formatTauriError(error: unknown, command: string): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }

  if (typeof error === "string" && error.trim()) {
    return error;
  }

  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string" &&
    error.message.trim()
  ) {
    return error.message;
  }

  try {
    const serialized = JSON.stringify(error);
    if (serialized && serialized !== "{}") {
      return `Tauri command '${command}' failed: ${serialized}`;
    }
  } catch {
    // Ignore serialization failures and fall through to the generic message.
  }

  return `Tauri command '${command}' failed for an unknown reason.`;
}

export interface DesktopClient {
  getBootstrap(): Promise<BootstrapDto>;
  getConfig(): Promise<AppConfigDto>;
  updateConfig(request: ConfigUpdateRequest): Promise<AppConfigDto>;
  getDaemonStatus(): Promise<DaemonStatusDto>;
  listDomains(): Promise<DomainInfoDto[]>;
  listProjects(): Promise<ProjectInfoDto[]>;
  getAvailablePort(): Promise<AvailablePortDto>;
  detectProjectInitPlan(
    request: DetectProjectInitPlanRequest,
  ): Promise<DetectProjectInitPlanDto>;
  installRoxy(): Promise<InstallResultDto>;
  setupCliAccess(): Promise<CliAccessDto>;
  listPortConflicts(): Promise<PortConflictDto[]>;
  killPortConflict(request: KillPortConflictRequest): Promise<PortConflictDto>;
  getUninstallPreview(): Promise<UninstallPreviewDto>;
  uninstallRoxy(): Promise<UninstallResultDto>;
  createProject(request: CreateProjectRequest): Promise<ProjectInfoDto>;
  createProjectWithDomains(
    request: CreateProjectWithDomainsRequest,
  ): Promise<ProjectInfoDto>;
  initProjectFromFolder(
    request: InitProjectFromFolderRequest,
  ): Promise<InitProjectFromFolderResultDto>;
  updateProject(request: UpdateProjectRequest): Promise<ProjectInfoDto>;
  removeProject(request: RemoveProjectRequest): Promise<RemoveProjectResultDto>;
  assignDomainProject(request: AssignDomainProjectRequest): Promise<DomainInfoDto>;
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
  openInEditor(request: OpenInEditorRequest): Promise<void>;
}

export function createDesktopClient(): DesktopClient {
  return {
    getBootstrap: () => tauriInvoke("get_bootstrap"),
    getConfig: () => tauriInvoke("get_config"),
    updateConfig: (request) => tauriInvoke("update_config", { request }),
    getDaemonStatus: () => tauriInvoke("get_daemon_status"),
    listDomains: () => tauriInvoke("list_domains"),
    listProjects: () => tauriInvoke("list_projects"),
    getAvailablePort: () => tauriInvoke("get_available_port"),
    detectProjectInitPlan: (request) =>
      tauriInvoke("detect_project_init_plan", {
        request: detectProjectInitPlanRequestSchema.parse(request),
      }),
    installRoxy: () => tauriInvoke("install_roxy"),
    setupCliAccess: () => tauriInvoke("setup_cli_access"),
    listPortConflicts: () => tauriInvoke("list_port_conflicts"),
    killPortConflict: (request) =>
      tauriInvoke("kill_port_conflict", {
        request: killPortConflictRequestSchema.parse(request),
      }),
    getUninstallPreview: () => tauriInvoke("get_uninstall_preview"),
    uninstallRoxy: () => tauriInvoke("uninstall_roxy"),
    createProject: (request) => tauriInvoke("create_project", { request }),
    createProjectWithDomains: (request) =>
      tauriInvoke("create_project_with_domains", {
        request: createProjectWithDomainsRequestSchema.parse(request),
      }),
    initProjectFromFolder: (request) =>
      tauriInvoke("init_project_from_folder", {
        request: initProjectFromFolderRequestSchema.parse(request),
      }),
    updateProject: (request) => tauriInvoke("update_project", { request }),
    removeProject: (request) =>
      tauriInvoke("remove_project", {
        request: removeProjectRequestSchema.parse(request),
      }),
    assignDomainProject: (request) =>
      tauriInvoke("assign_domain_project", { request }),
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
    openInEditor: (request) =>
      tauriInvoke("open_in_editor", {
        request: openInEditorRequestSchema.parse(request),
      }),
  };
}

export const tauriClient = createDesktopClient();
