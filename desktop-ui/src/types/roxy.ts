export interface NodeTrustDto {
  ready: boolean;
  alreadyReady: boolean;
  caCertPath: string;
  shellSnippetPath: string;
  shellIntegrationReady: boolean;
  launchctlReady: boolean;
  lastError: string | null;
}

export interface EnvironmentDto {
  appName: string;
  desktopName: string;
  configPath: string;
  binaryPath: string;
  binaryExists: boolean;
  fixedDomainSuffix: string;
  cliHelperReady: boolean;
  cliAccessReady: boolean;
  cliCommandPath: string;
  nodeTrust: NodeTrustDto;
}

export interface DaemonConfigDto {
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
  logLevel: string;
}

export interface RoxyPathsDto {
  dataDir: string;
  pidFile: string;
  logFile: string;
  certsDir: string;
  socketPath: string;
}

export interface DockerConfigDto {
  enabled: boolean;
}

export interface AppConfigDto {
  configPath: string;
  daemon: DaemonConfigDto;
  paths: RoxyPathsDto;
  docker: DockerConfigDto;
}

export interface ConfigUpdateRequest {
  daemon?: DaemonConfigDto;
  paths?: RoxyPathsDto;
  docker?: DockerConfigDto;
}

export interface RouteDto {
  path: string;
  target: string;
  targetKind: string;
}

export interface RegistrationDto {
  pattern: string;
  domain: string;
  wildcard: boolean;
  httpsEnabled: boolean;
  source: string;
  routes: RouteDto[];
}

export interface DomainInfoDto {
  registration: RegistrationDto;
  hasCert: boolean;
  certTrusted: boolean | null;
  projectId: string | null;
  projectName: string | null;
  projectEnabled: boolean;
  active: boolean;
  targetReachable: boolean;
}

export interface ProjectInfoDto {
  id: string;
  name: string;
  enabled: boolean;
  domainCount: number;
  hasReachableDomain: boolean;
  rootPath: string | null;
}

export interface DnsResolutionDto {
  checked: boolean;
  healthy: boolean;
  hostname: string;
  resolvedIps: string[];
  message: string | null;
}

export interface DaemonStatusDto {
  running: boolean;
  pid: number | null;
  lanIp: string;
  caInstalled: boolean;
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
  dnsResolution: DnsResolutionDto;
}

export interface BootstrapDto {
  environment: EnvironmentDto;
  config: AppConfigDto;
  daemon: DaemonStatusDto;
  licensing: LicensingDto;
  launcherPreferences: LauncherPreferencesDto;
  launcherAvailability: LauncherAvailabilityDto;
  domains: DomainInfoDto[];
  projects: ProjectInfoDto[];
}

export interface AutostartStatusDto {
  enabled: boolean;
}

export type EditorPreference =
  | "auto"
  | "cursor"
  | "vsCode"
  | "windsurf"
  | "zed"
  | "sublimeText"
  | "nova";
export type TerminalPreference = "terminal" | "ghostty" | "iTerm" | "wezTerm";
export type BrowserPreference =
  | "default"
  | "helium"
  | "arc"
  | "chrome"
  | "safari"
  | "firefox"
  | "brave";
export type RunActionPreference = "terminalOnly" | "editorAndTerminal";

export interface LauncherPreferencesDto {
  editor: EditorPreference;
  terminal: TerminalPreference;
  browser: BrowserPreference;
  runAction: RunActionPreference;
}

export interface EditorOptionDto {
  id: Exclude<EditorPreference, "auto">;
  label: string;
}

export interface TerminalOptionDto {
  id: TerminalPreference;
  label: string;
}

export interface LauncherAvailabilityDto {
  availableEditors: EditorOptionDto[];
  availableTerminals: TerminalOptionDto[];
}

export interface SettingsTransferSettingsDto {
  daemon: DaemonConfigDto;
  docker: DockerConfigDto;
  keygen: KeygenSettingsUpdateRequest;
  launcherPreferences: LauncherPreferencesDto;
}

export interface SettingsTransferDomainDto {
  domain: string;
  wildcard: boolean;
  routes: string[];
}

export interface SettingsTransferProjectDto {
  id: string;
  name: string;
  enabled: boolean;
  rootPath: string | null;
  domains: SettingsTransferDomainDto[];
}

export interface SettingsTransferSnapshotDto {
  version: number;
  scope: "all" | "projects" | string;
  settings: SettingsTransferSettingsDto | null;
  standaloneDomains: SettingsTransferDomainDto[];
  projects: SettingsTransferProjectDto[];
}

export interface SettingsExportRequest {
  filePath: string;
  mode: "all" | "projects";
  projectIds: string[];
}

export interface SettingsExportResultDto {
  filePath: string;
  mode: "all" | "projects" | string;
  includesSettings: boolean;
  standaloneDomainCount: number;
  projectCount: number;
}

export interface SettingsImportPreviewRequest {
  filePath: string;
}

export interface SettingsImportPreviewProjectDto {
  id: string;
  name: string;
  enabled: boolean;
  domainCount: number;
  hasRootPath: boolean;
}

export interface SettingsImportPreviewDto {
  filePath: string;
  version: number;
  scope: "all" | "projects" | string;
  includesSettings: boolean;
  standaloneDomainCount: number;
  projectCount: number;
  projects: SettingsImportPreviewProjectDto[];
}

export interface SettingsImportRequest {
  filePath: string;
  mode: "replaceAll" | "projects";
  projectIds: string[];
}

export interface SettingsImportResultDto {
  mode: "replaceAll" | "projects" | string;
  importedProjects: number;
  importedDomains: number;
  importedStandaloneDomains: number;
  appliedSettings: boolean;
  skippedRootPaths: number;
  daemonReloaded: boolean;
  daemonReloadError: string | null;
}

export interface LicenseLimitsDto {
  plan: string;
  maxProjects: number | null;
  maxDomainsPerProject: number | null;
}

export interface MachineDto {
  fingerprint: string;
  name: string | null;
  hostname: string | null;
  platform: string;
}

export interface ActiveLicenseDto {
  keyMasked: string;
  licenseId: string;
  name: string | null;
  expiry: string | null;
  lastValidatedAt: string | null;
  validationCode: string | null;
  validationDetail: string | null;
  machineId: string | null;
  machineFingerprint: string | null;
  machineName: string | null;
  machineHostname: string | null;
  machinePlatform: string | null;
  responseSignatureVerified: boolean;
  responseSignatureDate: string | null;
}

export interface OfflineLicenseDto {
  algorithm: string;
  issued: string | null;
  fileExpiry: string | null;
  licenseId: string | null;
  name: string | null;
  licenseExpiry: string | null;
  plan: string | null;
  maxProjects: number | null;
  maxDomainsPerProject: number | null;
}

export interface LicensingDto {
  status:
    | "notConfigured"
    | "unlicensed"
    | "licensed"
    | "licensedOffline"
    | "invalid"
    | string;
  configured: boolean;
  apiUrl: string;
  account: string;
  product: string | null;
  publicKey: string | null;
  limits: LicenseLimitsDto;
  freeLimits: LicenseLimitsDto;
  projectCount: number;
  domainCount: number;
  lastError: string | null;
  currentMachine: MachineDto | null;
  activeLicense: ActiveLicenseDto | null;
  offlineCachePresent: boolean;
  offlineLicense: OfflineLicenseDto | null;
}

export interface AppUpdateInfoDto {
  version: string;
  notes: string | null;
  pubDate: string | null;
}

export interface AppUpdateCheckDto {
  configured: boolean;
  update: AppUpdateInfoDto | null;
}

export type DeepLinkRouteDto = "projects" | "logs" | "settingsLicensing";

export interface DeepLinkPayloadDto {
  route: DeepLinkRouteDto;
  projectId: string | null;
  view: string | null;
}

export interface StepDto {
  label: string | null;
  status: "success" | "warning" | "skipped" | string;
  message: string;
}

export interface InstallResultDto {
  lanIp: string;
  steps: StepDto[];
}

export interface CliAccessDto {
  ready: boolean;
  commandPath: string;
  alreadyReady: boolean;
}

export interface PortConflictDto {
  protocol: "http" | "https" | "dns";
  port: number;
  pid: number;
  processName: string;
  user: string;
  command: string;
}

export interface KillPortConflictRequest {
  protocol: "http" | "https" | "dns";
  port: number;
  pid: number;
}

export interface UninstallPreviewDto {
  domainCount: number;
  dataDir: string;
}

export interface UninstallResultDto {
  steps: StepDto[];
}

export interface RegisterDomainRequest {
  domain: string;
  wildcard: boolean;
  routes: string[];
  projectId?: string | null;
}

export interface UpdateDomainRequest extends RegisterDomainRequest {
  currentDomain: string;
  currentWildcard: boolean;
}

export interface KeygenSettingsUpdateRequest {
  apiUrl: string;
  account: string;
  product?: string | null;
  publicKey?: string | null;
}

export interface ActivateLicenseRequest {
  key: string;
}

export interface ImportOfflineLicenseRequest {
  certificate: string;
  key?: string | null;
}

export interface CreateProjectRequest {
  name: string;
}

export interface CreateProjectDomainRequest {
  domain: string;
  wildcard: boolean;
  routes: string[];
}

export interface CreateProjectWithDomainsRequest {
  name: string;
  domains?: CreateProjectDomainRequest[];
}

export interface AssignDomainProjectRequest {
  domain: string;
  wildcard: boolean;
  projectId?: string | null;
}

export interface UpdateProjectRequest {
  id: string;
  name?: string;
  enabled?: boolean;
  rootPath?: string | null;
}

export interface RemoveProjectRequest {
  id: string;
  deleteDomains: boolean;
}

export interface RemoveProjectResultDto {
  project: ProjectInfoDto;
  removedDomains: string[];
  daemonReloaded: boolean;
}

export interface AvailablePortDto {
  port: number;
}

export interface DetectProjectInitPlanRequest {
  rootPath: string;
}

export interface FolderInitServiceDto {
  id: string;
  label: string;
  displayPath: string;
  domain: string;
  target: string;
  enabled: boolean;
  canUpdateDevScript: boolean;
  updateDevScript: boolean;
}

export interface DetectProjectInitPlanDto {
  rootPath: string;
  name: string;
  services: FolderInitServiceDto[];
}

export interface InitProjectFromFolderServiceRequest {
  id: string;
  domain: string;
  target: string;
  enabled: boolean;
  updateDevScript: boolean;
}

export interface InitProjectFromFolderRequest {
  rootPath: string;
  name: string;
  services: InitProjectFromFolderServiceRequest[];
}

export interface InitProjectServiceRequest {
  domain: string;
  target: string;
}

export interface OpenInEditorRequest {
  path: string;
}

export interface OpenProjectUrlRequest {
  url: string;
  browser: BrowserPreference;
}

export interface RunLocalDevServerRequest {
  rootPath: string;
  target: string;
}

export interface StopLocalDevServerRequest {
  target: string;
}

export interface UpdateLauncherPreferencesRequest {
  editor: EditorPreference;
  terminal: TerminalPreference;
  browser: BrowserPreference;
  runAction: RunActionPreference;
}

export interface UpdateAutostartRequest {
  enabled: boolean;
}

export interface InitProjectFromFolderResultDto {
  project: ProjectInfoDto;
  services: InitProjectServiceRequest[];
  daemonReloaded: boolean;
  updatedFiles: string[];
  skippedFileUpdates: string[];
}

export interface DomainPatternRequest {
  domain: string;
  wildcard: boolean;
}

export interface AddRouteRequest {
  domain: string;
  wildcard: boolean;
  path: string;
  target: string;
}

export interface RemoveRouteRequest {
  domain: string;
  wildcard: boolean;
  path: string;
}

export interface RegistrationMutationDto {
  registration: RegistrationDto;
  route: RouteDto | null;
}

export interface RegisterDomainResultDto {
  registration: RegistrationDto;
  certOutcome: StepDto;
}

export interface UnregisterDomainResultDto {
  registration: RegistrationDto;
  certOutcome: StepDto;
}

export interface StartDaemonRequest {
  verbose: boolean;
}

export interface DaemonLaunchDto {
  pid: number;
  lanIp: string;
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
}

export interface LogsRequest {
  lines: number;
  clear: boolean;
}

export interface LogsDto {
  path: string;
  exists: boolean;
  cleared: boolean;
  content: string;
}
