import { invoke } from "@tauri-apps/api/core";

import { createProjectWithDomainsRequestSchema } from "./validators/validator.create-project";
import {
  detectProjectInitPlanRequestSchema,
  initProjectFromFolderRequestSchema,
} from "./validators/validator.init-project-from-folder";
import {
  activateLicenseRequestSchema,
  importOfflineLicenseRequestSchema,
  keygenSettingsUpdateRequestSchema,
} from "./validators/validator.license";
import { launcherPreferencesSchema } from "./validators/validator.launcher-preferences";
import { openInEditorRequestSchema } from "./validators/validator.open-in-editor";
import { killPortConflictRequestSchema } from "./validators/validator.port-conflict";
import { removeProjectRequestSchema } from "./validators/validator.remove-project";
import { runLocalDevServerRequestSchema } from "./validators/validator.run-local-dev-server";
import { stopLocalDevServerRequestSchema } from "./validators/validator.stop-local-dev-server";
import {
  settingsExportRequestSchema,
  settingsImportPreviewRequestSchema,
  settingsImportRequestSchema,
} from "./validators/validator.settings-transfer";
import type {
  ActivateLicenseRequest,
  AddRouteRequest,
  AssignDomainProjectRequest,
  AvailablePortDto,
  AppConfigDto,
  DeepLinkPayloadDto,
  AppUpdateCheckDto,
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
  ImportOfflineLicenseRequest,
  InitProjectFromFolderRequest,
  InitProjectFromFolderResultDto,
  InstallResultDto,
  KeygenSettingsUpdateRequest,
  KillPortConflictRequest,
  LauncherPreferencesDto,
  LicensingDto,
  LogsDto,
  LogsRequest,
  NodeTrustDto,
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
  RunLocalDevServerRequest,
  StopLocalDevServerRequest,
  SettingsExportRequest,
  SettingsExportResultDto,
  SettingsImportPreviewDto,
  SettingsImportPreviewRequest,
  SettingsImportRequest,
  SettingsImportResultDto,
  StartDaemonRequest,
  UninstallPreviewDto,
  UninstallResultDto,
  UnregisterDomainResultDto,
  UpdateLauncherPreferencesRequest,
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
  checkAppUpdate(): Promise<AppUpdateCheckDto>;
  installAppUpdate(): Promise<void>;
  consumePendingDeepLink(): Promise<DeepLinkPayloadDto | null>;
  getConfig(): Promise<AppConfigDto>;
  updateConfig(request: ConfigUpdateRequest): Promise<AppConfigDto>;
  getLicensingStatus(): Promise<LicensingDto>;
  exportSettings(request: SettingsExportRequest): Promise<SettingsExportResultDto>;
  previewSettingsImport(
    request: SettingsImportPreviewRequest,
  ): Promise<SettingsImportPreviewDto>;
  importSettings(request: SettingsImportRequest): Promise<SettingsImportResultDto>;
  updateKeygenSettings(request: KeygenSettingsUpdateRequest): Promise<LicensingDto>;
  activateLicense(request: ActivateLicenseRequest): Promise<LicensingDto>;
  importOfflineLicense(request: ImportOfflineLicenseRequest): Promise<LicensingDto>;
  clearLicense(): Promise<LicensingDto>;
  refreshLicense(): Promise<LicensingDto>;
  getDaemonStatus(): Promise<DaemonStatusDto>;
  listDomains(): Promise<DomainInfoDto[]>;
  listProjects(): Promise<ProjectInfoDto[]>;
  getAvailablePort(): Promise<AvailablePortDto>;
  detectProjectInitPlan(
    request: DetectProjectInitPlanRequest,
  ): Promise<DetectProjectInitPlanDto>;
  installRoxy(): Promise<InstallResultDto>;
  setupCliAccess(): Promise<CliAccessDto>;
  setupNodeTrust(): Promise<NodeTrustDto>;
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
  runLocalDevServer(request: RunLocalDevServerRequest): Promise<void>;
  stopLocalDevServer(request: StopLocalDevServerRequest): Promise<void>;
  updateLauncherPreferences(
    request: UpdateLauncherPreferencesRequest,
  ): Promise<LauncherPreferencesDto>;
}

export function createDesktopClient(): DesktopClient {
  return {
    getBootstrap: () => tauriInvoke("get_bootstrap"),
    checkAppUpdate: () => tauriInvoke("check_app_update"),
    installAppUpdate: () => tauriInvoke("install_app_update"),
    consumePendingDeepLink: () => tauriInvoke("consume_pending_deep_link"),
    getConfig: () => tauriInvoke("get_config"),
    updateConfig: (request) => tauriInvoke("update_config", { request }),
    getLicensingStatus: () => tauriInvoke("get_licensing_status"),
    exportSettings: (request) =>
      tauriInvoke("export_settings", {
        request: settingsExportRequestSchema.parse(request),
      }),
    previewSettingsImport: (request) =>
      tauriInvoke("preview_settings_import", {
        request: settingsImportPreviewRequestSchema.parse(request),
      }),
    importSettings: (request) =>
      tauriInvoke("import_settings", {
        request: settingsImportRequestSchema.parse(request),
      }),
    updateKeygenSettings: (request) =>
      tauriInvoke("update_keygen_settings", {
        request: keygenSettingsUpdateRequestSchema.parse(request),
      }),
    activateLicense: (request) =>
      tauriInvoke("activate_license", {
        request: activateLicenseRequestSchema.parse(request),
      }),
    importOfflineLicense: (request) =>
      tauriInvoke("import_offline_license", {
        request: importOfflineLicenseRequestSchema.parse(request),
      }),
    clearLicense: () => tauriInvoke("clear_license"),
    refreshLicense: () => tauriInvoke("refresh_license"),
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
    setupNodeTrust: () => tauriInvoke("setup_node_trust"),
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
    runLocalDevServer: (request) =>
      tauriInvoke("run_local_dev_server", {
        request: runLocalDevServerRequestSchema.parse(request),
      }),
    stopLocalDevServer: (request) =>
      tauriInvoke("stop_local_dev_server", {
        request: stopLocalDevServerRequestSchema.parse(request),
      }),
    updateLauncherPreferences: (request) =>
      tauriInvoke("update_launcher_preferences", {
        request: launcherPreferencesSchema.parse(request),
      }),
  };
}

export const tauriClient = createDesktopClient();
