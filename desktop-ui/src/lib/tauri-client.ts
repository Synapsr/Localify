import { invoke } from "@tauri-apps/api/core";

import type {
  AddProjectServiceRequest,
  AddProjectServiceResultDto,
  AddRouteRequest,
  AgentMcpInstallResultDto,
  AgentMcpStatusDto,
  AssignDomainProjectRequest,
  AvailablePortDto,
  AppConfigDto,
  AutostartStatusDto,
  DeepLinkPayloadDto,
  AppUpdateCheckDto,
  BootstrapDto,
  CliAccessDto,
  CloudAccountDto,
  CloudSignInPollDto,
  CloudSignInStartDto,
  ConfigUpdateRequest,
  CreateProjectRequest,
  CreateProjectWithDomainsRequest,
  CreatePublicUrlRequest,
  DaemonLaunchDto,
  DaemonStatusDto,
  DetectProjectInitPlanDto,
  DetectProjectInitPlanRequest,
  DiscoverProjectWorktreesRequest,
  DomainInfoDto,
  DomainPatternRequest,
  AttachProjectWorktreeRequest,
  AttachProjectWorktreeResultDto,
  EnvironmentFileCopyPreviewDto,
  EnvironmentFileCopyRequest,
  EnvironmentFileDto,
  EnvironmentFileListDto,
  EnvironmentFileRequest,
  InitProjectFromFolderRequest,
  InitProjectFromFolderResultDto,
  InstallResultDto,
  KillPortConflictRequest,
  LauncherPreferencesDto,
  LogsDto,
  LogsRequest,
  NodeTrustDto,
  OpenEnvInEditorRequest,
  OpenInEditorRequest,
  OpenProjectUrlRequest,
  PollCloudSignInRequest,
  PortConflictDto,
  ProjectInfoDto,
  ProjectWorktreeDiscoveryDto,
  PublicUrlDto,
  PublicUrlRequest,
  PublicUrlWithPasswordDto,
  RegisterDomainRequest,
  RegisterDomainResultDto,
  RegistrationDto,
  RegistrationMutationDto,
  RelinkProjectRootRequest,
  RemoveProjectRequest,
  RemoveProjectEnvironmentRequest,
  RemoveProjectEnvironmentResultDto,
  RemoveProjectResultDto,
  RemoveRouteRequest,
  RunLocalDevServerRequest,
  SaveEnvironmentFileRequest,
  SetPublicUrlProtectionRequest,
  StopLocalDevServerRequest,
  SettingsExportRequest,
  SettingsExportResultDto,
  SettingsImportPreviewDto,
  SettingsImportPreviewRequest,
  SettingsImportRequest,
  SettingsImportResultDto,
  StartDaemonRequest,
  StepDto,
  UninstallPreviewDto,
  UninstallResultDto,
  TerminalCreateSessionRequest,
  TerminalLaunchServiceRequest,
  TerminalLaunchServiceResultDto,
  TerminalResizeRequest,
  TerminalSessionDto,
  TerminalSessionRequest,
  TerminalWriteRequest,
  UpdateAutostartRequest,
  UnregisterDomainResultDto,
  UpdateDomainRequest,
  UpdateLauncherPreferencesRequest,
  UpdateProjectRequest,
} from "../types/localify";

import { addProjectServiceRequestSchema } from "./validators/validator.add-project-service";
import { updateAutostartRequestSchema } from "./validators/validator.autostart";
import {
  createPublicUrlRequestSchema,
  pollCloudSignInRequestSchema,
  publicUrlRequestSchema,
  setPublicUrlProtectionRequestSchema,
} from "./validators/validator.cloud";
import {
  createProjectWithDomainsRequestSchema,
  registerDomainRequestSchema,
  updateDomainRequestSchema,
} from "./validators/validator.create-project";
import {
  environmentFileRequestSchema,
  saveEnvironmentFileRequestSchema,
} from "./validators/validator.environment-file";
import {
  detectProjectInitPlanRequestSchema,
  initProjectFromFolderRequestSchema,
} from "./validators/validator.init-project-from-folder";
import { launcherPreferencesSchema } from "./validators/validator.launcher-preferences";
import { openEnvInEditorRequestSchema } from "./validators/validator.open-env-in-editor";
import { openInEditorRequestSchema } from "./validators/validator.open-in-editor";
import { openProjectUrlRequestSchema } from "./validators/validator.open-project-url";
import { killPortConflictRequestSchema } from "./validators/validator.port-conflict";
import {
  attachProjectWorktreeRequestSchema,
  discoverProjectWorktreesRequestSchema,
  environmentFileCopyRequestSchema,
} from "./validators/validator.project-worktree";
import { relinkProjectRootRequestSchema } from "./validators/validator.relink-project-root";
import { removeProjectRequestSchema } from "./validators/validator.remove-project";
import { removeProjectEnvironmentRequestSchema } from "./validators/validator.remove-project-environment";
import { runLocalDevServerRequestSchema } from "./validators/validator.run-local-dev-server";
import {
  settingsExportRequestSchema,
  settingsImportPreviewRequestSchema,
  settingsImportRequestSchema,
} from "./validators/validator.settings-transfer";
import { stopLocalDevServerRequestSchema } from "./validators/validator.stop-local-dev-server";
import {
  terminalCreateSessionRequestSchema,
  terminalLaunchServiceRequestSchema,
  terminalResizeRequestSchema,
  terminalSessionRequestSchema,
  terminalWriteRequestSchema,
} from "./validators/validator.terminal";

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
  getAutostartStatus(): Promise<AutostartStatusDto>;
  setAutostartEnabled(request: UpdateAutostartRequest): Promise<AutostartStatusDto>;
  checkAppUpdate(): Promise<AppUpdateCheckDto>;
  installAppUpdate(): Promise<void>;
  readInstallAttributionClipboard(): Promise<string | null>;
  consumePendingDeepLink(): Promise<DeepLinkPayloadDto | null>;
  getConfig(): Promise<AppConfigDto>;
  updateConfig(request: ConfigUpdateRequest): Promise<AppConfigDto>;
  exportSettings(request: SettingsExportRequest): Promise<SettingsExportResultDto>;
  previewSettingsImport(request: SettingsImportPreviewRequest): Promise<SettingsImportPreviewDto>;
  importSettings(request: SettingsImportRequest): Promise<SettingsImportResultDto>;
  getDaemonStatus(): Promise<DaemonStatusDto>;
  listDomains(): Promise<DomainInfoDto[]>;
  listProjects(): Promise<ProjectInfoDto[]>;
  getAvailablePort(): Promise<AvailablePortDto>;
  detectProjectInitPlan(request: DetectProjectInitPlanRequest): Promise<DetectProjectInitPlanDto>;
  installLocalify(): Promise<InstallResultDto>;
  setupCliAccess(): Promise<CliAccessDto>;
  setupNodeTrust(): Promise<NodeTrustDto>;
  trustCa(): Promise<void>;
  trustIosSimulators(): Promise<StepDto>;
  getAgentMcpStatus(): Promise<AgentMcpStatusDto>;
  installAgentMcp(): Promise<AgentMcpInstallResultDto>;
  connectAgentsOnce(): Promise<AgentMcpInstallResultDto | null>;
  getCloudAccount(): Promise<CloudAccountDto>;
  startCloudSignIn(): Promise<CloudSignInStartDto>;
  pollCloudSignIn(request: PollCloudSignInRequest): Promise<CloudSignInPollDto>;
  signOutCloud(): Promise<CloudAccountDto>;
  listPublicUrls(): Promise<PublicUrlDto[]>;
  createPublicUrl(request: CreatePublicUrlRequest): Promise<PublicUrlWithPasswordDto>;
  removePublicUrl(request: PublicUrlRequest): Promise<void>;
  setPublicUrlProtection(request: SetPublicUrlProtectionRequest): Promise<PublicUrlWithPasswordDto>;
  claimPublicUrl(request: PublicUrlRequest): Promise<PublicUrlDto>;
  listPortConflicts(): Promise<PortConflictDto[]>;
  killPortConflict(request: KillPortConflictRequest): Promise<PortConflictDto>;
  getUninstallPreview(): Promise<UninstallPreviewDto>;
  uninstallLocalify(): Promise<UninstallResultDto>;
  createProject(request: CreateProjectRequest): Promise<ProjectInfoDto>;
  createProjectWithDomains(request: CreateProjectWithDomainsRequest): Promise<ProjectInfoDto>;
  initProjectFromFolder(
    request: InitProjectFromFolderRequest,
  ): Promise<InitProjectFromFolderResultDto>;
  addProjectService(request: AddProjectServiceRequest): Promise<AddProjectServiceResultDto>;
  updateProject(request: UpdateProjectRequest): Promise<ProjectInfoDto>;
  relinkProjectRoot(request: RelinkProjectRootRequest): Promise<ProjectInfoDto>;
  removeProject(request: RemoveProjectRequest): Promise<RemoveProjectResultDto>;
  removeProjectEnvironment(
    request: RemoveProjectEnvironmentRequest,
  ): Promise<RemoveProjectEnvironmentResultDto>;
  discoverProjectWorktrees(
    request: DiscoverProjectWorktreesRequest,
  ): Promise<ProjectWorktreeDiscoveryDto>;
  attachProjectWorktree(
    request: AttachProjectWorktreeRequest,
  ): Promise<AttachProjectWorktreeResultDto>;
  assignDomainProject(request: AssignDomainProjectRequest): Promise<DomainInfoDto>;
  registerDomain(request: RegisterDomainRequest): Promise<RegisterDomainResultDto>;
  updateDomain(request: UpdateDomainRequest): Promise<RegisterDomainResultDto>;
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
  openEnvInEditor(request: OpenEnvInEditorRequest): Promise<void>;
  listEnvironmentFiles(request: EnvironmentFileRequest): Promise<EnvironmentFileListDto>;
  readEnvironmentFile(request: EnvironmentFileRequest): Promise<EnvironmentFileDto>;
  saveEnvironmentFile(request: SaveEnvironmentFileRequest): Promise<EnvironmentFileDto>;
  previewEnvironmentFileCopy(
    request: EnvironmentFileCopyRequest,
  ): Promise<EnvironmentFileCopyPreviewDto>;
  copyEnvironmentFiles(request: EnvironmentFileCopyRequest): Promise<EnvironmentFileCopyPreviewDto>;
  openProjectUrl(request: OpenProjectUrlRequest): Promise<void>;
  runLocalDevServer(request: RunLocalDevServerRequest): Promise<void>;
  stopLocalDevServer(request: StopLocalDevServerRequest): Promise<void>;
  updateLauncherPreferences(
    request: UpdateLauncherPreferencesRequest,
  ): Promise<LauncherPreferencesDto>;
  terminalListSessions(): Promise<TerminalSessionDto[]>;
  terminalCreateSession(request: TerminalCreateSessionRequest): Promise<TerminalSessionDto>;
  terminalLaunchService(
    request: TerminalLaunchServiceRequest,
  ): Promise<TerminalLaunchServiceResultDto>;
  terminalWrite(request: TerminalWriteRequest): Promise<void>;
  terminalResize(request: TerminalResizeRequest): Promise<void>;
  terminalInterrupt(request: TerminalSessionRequest): Promise<void>;
  terminalClose(request: TerminalSessionRequest): Promise<void>;
}

export function createDesktopClient(): DesktopClient {
  return {
    getBootstrap: () => tauriInvoke("get_bootstrap"),
    getAutostartStatus: () => tauriInvoke("get_autostart_status"),
    setAutostartEnabled: (request) =>
      tauriInvoke("set_autostart_enabled", {
        request: updateAutostartRequestSchema.parse(request),
      }),
    checkAppUpdate: () => tauriInvoke("check_app_update"),
    installAppUpdate: () => tauriInvoke("install_app_update"),
    readInstallAttributionClipboard: () => tauriInvoke("read_install_attribution_clipboard"),
    consumePendingDeepLink: () => tauriInvoke("consume_pending_deep_link"),
    getConfig: () => tauriInvoke("get_config"),
    updateConfig: (request) => tauriInvoke("update_config", { request }),
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
    getDaemonStatus: () => tauriInvoke("get_daemon_status"),
    listDomains: () => tauriInvoke("list_domains"),
    listProjects: () => tauriInvoke("list_projects"),
    getAvailablePort: () => tauriInvoke("get_available_port"),
    detectProjectInitPlan: (request) =>
      tauriInvoke("detect_project_init_plan", {
        request: detectProjectInitPlanRequestSchema.parse(request),
      }),
    installLocalify: () => tauriInvoke("install_localify"),
    setupCliAccess: () => tauriInvoke("setup_cli_access"),
    setupNodeTrust: () => tauriInvoke("setup_node_trust"),
    trustCa: () => tauriInvoke("trust_ca"),
    trustIosSimulators: () => tauriInvoke<StepDto>("trust_ios_simulators"),
    getAgentMcpStatus: () => tauriInvoke("get_agent_mcp_status"),
    installAgentMcp: () => tauriInvoke("install_agent_mcp"),
    connectAgentsOnce: () => tauriInvoke("connect_agents_once"),
    getCloudAccount: () => tauriInvoke("get_cloud_account"),
    startCloudSignIn: () => tauriInvoke("start_cloud_sign_in"),
    pollCloudSignIn: (request) =>
      tauriInvoke("poll_cloud_sign_in", {
        request: pollCloudSignInRequestSchema.parse(request),
      }),
    signOutCloud: () => tauriInvoke("sign_out_cloud"),
    listPublicUrls: () => tauriInvoke("list_public_urls"),
    createPublicUrl: (request) =>
      tauriInvoke("create_public_url", {
        request: createPublicUrlRequestSchema.parse(request),
      }),
    removePublicUrl: (request) =>
      tauriInvoke("remove_public_url", {
        request: publicUrlRequestSchema.parse(request),
      }),
    setPublicUrlProtection: (request) =>
      tauriInvoke("set_public_url_protection", {
        request: setPublicUrlProtectionRequestSchema.parse(request),
      }),
    claimPublicUrl: (request) =>
      tauriInvoke("claim_public_url", {
        request: publicUrlRequestSchema.parse(request),
      }),
    listPortConflicts: () => tauriInvoke("list_port_conflicts"),
    killPortConflict: (request) =>
      tauriInvoke("kill_port_conflict", {
        request: killPortConflictRequestSchema.parse(request),
      }),
    getUninstallPreview: () => tauriInvoke("get_uninstall_preview"),
    uninstallLocalify: () => tauriInvoke("uninstall_localify"),
    createProject: (request) => tauriInvoke("create_project", { request }),
    createProjectWithDomains: (request) =>
      tauriInvoke("create_project_with_domains", {
        request: createProjectWithDomainsRequestSchema.parse(request),
      }),
    initProjectFromFolder: (request) =>
      tauriInvoke("init_project_from_folder", {
        request: initProjectFromFolderRequestSchema.parse(request),
      }),
    addProjectService: (request) =>
      tauriInvoke("add_project_service", {
        request: addProjectServiceRequestSchema.parse(request),
      }),
    updateProject: (request) => tauriInvoke("update_project", { request }),
    relinkProjectRoot: (request) =>
      tauriInvoke("relink_project_root", {
        request: relinkProjectRootRequestSchema.parse(request),
      }),
    removeProject: (request) =>
      tauriInvoke("remove_project", {
        request: removeProjectRequestSchema.parse(request),
      }),
    removeProjectEnvironment: (request) =>
      tauriInvoke("remove_project_environment", {
        request: removeProjectEnvironmentRequestSchema.parse(request),
      }),
    discoverProjectWorktrees: (request) =>
      tauriInvoke("discover_project_worktrees", {
        request: discoverProjectWorktreesRequestSchema.parse(request),
      }),
    attachProjectWorktree: (request) =>
      tauriInvoke("attach_project_worktree", {
        request: attachProjectWorktreeRequestSchema.parse(request),
      }),
    assignDomainProject: (request) => tauriInvoke("assign_domain_project", { request }),
    registerDomain: (request) =>
      tauriInvoke("register_domain", {
        request: registerDomainRequestSchema.parse(request),
      }),
    updateDomain: (request) =>
      tauriInvoke("update_domain", {
        request: updateDomainRequestSchema.parse(request),
      }),
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
    openEnvInEditor: (request) =>
      tauriInvoke("open_env_in_editor", {
        request: openEnvInEditorRequestSchema.parse(request),
      }),
    listEnvironmentFiles: (request) =>
      tauriInvoke("list_environment_files", {
        request: environmentFileRequestSchema.parse(request),
      }),
    readEnvironmentFile: (request) =>
      tauriInvoke("read_environment_file", {
        request: environmentFileRequestSchema.parse(request),
      }),
    saveEnvironmentFile: (request) =>
      tauriInvoke("save_environment_file", {
        request: saveEnvironmentFileRequestSchema.parse(request),
      }),
    previewEnvironmentFileCopy: (request) =>
      tauriInvoke("preview_environment_file_copy", {
        request: environmentFileCopyRequestSchema.parse(request),
      }),
    copyEnvironmentFiles: (request) =>
      tauriInvoke("copy_environment_files", {
        request: environmentFileCopyRequestSchema.parse(request),
      }),
    openProjectUrl: (request) =>
      tauriInvoke("open_project_url", {
        request: openProjectUrlRequestSchema.parse(request),
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
    terminalListSessions: () => tauriInvoke("terminal_list_sessions"),
    terminalCreateSession: (request) =>
      tauriInvoke("terminal_create_session", {
        request: terminalCreateSessionRequestSchema.parse(request),
      }),
    terminalLaunchService: (request) =>
      tauriInvoke("terminal_launch_service", {
        request: terminalLaunchServiceRequestSchema.parse(request),
      }),
    terminalWrite: (request) =>
      tauriInvoke("terminal_write", {
        request: terminalWriteRequestSchema.parse(request),
      }),
    terminalResize: (request) =>
      tauriInvoke("terminal_resize", {
        request: terminalResizeRequestSchema.parse(request),
      }),
    terminalInterrupt: (request) =>
      tauriInvoke("terminal_interrupt", {
        request: terminalSessionRequestSchema.parse(request),
      }),
    terminalClose: (request) =>
      tauriInvoke("terminal_close", {
        request: terminalSessionRequestSchema.parse(request),
      }),
  };
}

export const tauriClient = createDesktopClient();
