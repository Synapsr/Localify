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
  systemSetupRequired: boolean;
  cliAccessReady: boolean;
  cliCommandPath: string;
  nodeTrust: NodeTrustDto;
}

export interface AgentMcpAgentDto {
  id: string;
  label: string;
  detected: boolean;
  installed: boolean;
  manualCommand: string;
}

export interface AgentMcpStatusDto {
  serverName: string;
  serverCommand: string;
  agents: AgentMcpAgentDto[];
}

export interface AgentMcpInstallResultDto {
  status: AgentMcpStatusDto;
  installedAgents: string[];
  /** Copies of the old Localify skill that were removed. */
  removedSkills: string[];
}

export interface DaemonConfigDto {
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
  logLevel: string;
  lanAccess: boolean;
}

export interface LocalifyPathsDto {
  dataDir: string;
  pidFile: string;
  logFile: string;
  certsDir: string;
  socketPath: string;
}

export interface DockerConfigDto {
  enabled: boolean;
}

export interface AgentConfigDto {
  connectAgents: boolean;
  promoteDevScript: boolean;
}

export interface AppConfigDto {
  configPath: string;
  daemon: DaemonConfigDto;
  paths: LocalifyPathsDto;
  docker: DockerConfigDto;
  agent: AgentConfigDto;
}

export interface ConfigUpdateRequest {
  daemon?: DaemonConfigDto;
  paths?: LocalifyPathsDto;
  docker?: DockerConfigDto;
  agent?: AgentConfigDto;
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
  lastAccessedAt: number | null;
}

export interface ProjectInfoDto {
  id: string;
  name: string;
  enabled: boolean;
  domainCount: number;
  hasReachableDomain: boolean;
  rootPath: string | null;
  rootPathMissing: boolean;
  lastAccessedAt: number | null;
}

export interface ProjectServiceInfoDto {
  id: string;
  name: string;
  projectId: string;
  projectName: string;
  projectEnabled: boolean;
  primaryDomain: string;
  url: string;
  domains: string[];
  path: string | null;
  command: string[] | null;
  target: string | null;
  inferred: boolean;
}

export interface ProjectEnvironmentServiceInfoDto {
  serviceId: string;
  domain: string;
}

export interface LaunchProfileInfoDto {
  name: string;
  danger: boolean;
  /** Humanized, e.g. "4h". */
  maxDuration: string | null;
}

export interface ProjectEnvironmentInfoDto {
  projectId: string;
  projectName: string;
  name: string;
  slug: string;
  primary: boolean;
  rootPath: string | null;
  gitCommonDir: string | null;
  gitBranch: string | null;
  services: ProjectEnvironmentServiceInfoDto[];
  launchProfiles: LaunchProfileInfoDto[];
  /** Why `localify.toml` could not be read. */
  launchProfilesError: string | null;
}

export interface LaunchSessionDto {
  id: string;
  projectId: string;
  environmentSlug: string | null;
  serviceId: string;
  profile: string;
  danger: boolean;
  targetPort: number;
  /** Unix seconds. */
  startedAt: number;
  /** Unix seconds. */
  expiresAt: number | null;
  sources: string[];
  /** Tab title prefix and pill shown in the app's pages. */
  browserIndicator: boolean;
}

export interface DiscoverProjectWorktreesRequest {
  projectId: string;
}

export interface AttachProjectWorktreeRequest {
  projectId: string;
  rootPath: string;
  primaryRootPath?: string | null;
}

export interface AttachProjectWorktreeResultDto {
  environment: ProjectEnvironmentInfoDto;
  created: boolean;
  createdDomains: string[];
  daemonReloaded: boolean;
}

export type ProjectWorktreeStatusDto = "primary" | "attached" | "available" | "unavailable";

export interface ProjectWorktreeServiceCandidateDto {
  id: string;
  label: string;
  displayPath: string;
  servicePath: string | null;
}

export interface ProjectWorktreeDto {
  rootPath: string;
  name: string;
  slug: string;
  status: ProjectWorktreeStatusDto;
  attachedEnvironmentSlug: string | null;
  gitBranch: string | null;
  gitHead: string | null;
  gitCommonDir: string | null;
  existingEnvironmentFileCount: number;
  serviceCandidates: ProjectWorktreeServiceCandidateDto[];
  unavailableReason: string | null;
}

export interface ProjectWorktreeDiscoveryDto {
  projectId: string;
  projectName: string;
  primaryRootPath: string;
  worktrees: ProjectWorktreeDto[];
}

export type TerminalTargetKind = "project" | "projectEnvironment" | "service";

export interface TerminalTargetRequest {
  kind: TerminalTargetKind;
  projectId: string;
  environmentSlug: string;
  serviceId?: string | null;
}

export interface TerminalSessionTargetDto {
  kind: TerminalTargetKind;
  projectId: string;
  projectName: string;
  environmentSlug: string;
  environmentName: string;
  serviceId?: string | null;
  serviceName?: string | null;
}

export interface TerminalCreateSessionRequest {
  target: TerminalTargetRequest;
  cols: number;
  rows: number;
}

export interface TerminalLaunchServiceRequest {
  target: TerminalTargetRequest;
  cols: number;
  rows: number;
  /** Launch profile name; omitted or null launches with the default profile. */
  profile?: string | null;
}

export interface TerminalWriteRequest {
  sessionId: string;
  data: string;
}

export interface TerminalResizeRequest {
  sessionId: string;
  cols: number;
  rows: number;
}

export interface TerminalSessionRequest {
  sessionId: string;
}

export type TerminalSessionStatus = "running" | "exited";

export interface TerminalRestartServiceRequest {
  sessionId: string;
  cols: number;
  rows: number;
}

export interface TerminalSessionDto {
  id: string;
  title: string;
  workingDirectory: string;
  target: TerminalSessionTargetDto;
  status: TerminalSessionStatus;
  /** Set for a service tab: the run it shows. A shell tab has none. */
  run: TerminalRunDto | null;
}

export interface TerminalRunDto {
  runId: string;
  profile: string | null;
  /** Set once the run ended and its output was finalized. */
  ended: TerminalRunEndDto | null;
}

export interface TerminalRunEndDto {
  code: number | null;
  /** Whether `localify dev` registered the run before it ended. */
  registered: boolean;
  /** Ended by a signal (a stop, Ctrl-C) rather than on its own. */
  stopped: boolean;
  /** Unix seconds. */
  at: number;
  /** Why the run could not be ended or finalized. */
  error: string | null;
}

export interface TerminalLaunchServiceResultDto {
  session: TerminalSessionDto;
  /** The ended service tab this run took over. */
  replaced: string | null;
}

/** A service tab the app opened for a launch handed to it over its bridge. */
export interface TerminalOpenedPayload {
  session: TerminalSessionDto;
  /** The tab it took over. */
  replaced: string | null;
  /** The paired machine that asked, shown on the tab for a few seconds. */
  startedBy: string | null;
}

export type ShellEnvironmentState = "resolving" | "ready" | "failed";

export interface ShellEnvironmentDto {
  state: ShellEnvironmentState;
  shell: string;
  reason: string | null;
}

export type ServiceStopOutcome = "stopped" | "alreadyStopped" | "noRecord";

export interface ServiceStopDto {
  outcome: ServiceStopOutcome;
  /** Only for a service Localify has no record of: never over a run it knows. */
  offerPortStop: boolean;
}

export interface ServiceRunRequest {
  projectId: string;
  environmentSlug: string;
  serviceId: string;
}

export type ServiceRunState = "running" | "stopped" | "unknown";

export interface ServiceRunDto {
  projectId: string;
  environment: string | null;
  serviceId: string;
  runId: string | null;
  /** Started from a shell tab by an app from before run PTYs. */
  legacy: boolean;
  state: ServiceRunState;
  origin: "app" | "background" | "terminal";
  pid: number;
  /** Unix seconds. */
  startedAt: number;
  outputRunId: string | null;
}

export interface TerminalOutputPayload {
  sessionId: string;
  bytes: number[];
}

export interface TerminalExitPayload {
  sessionId: string;
  exitCode: number;
  signal: string | null;
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
  caTrustPending: boolean;
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
  dnsResolution: DnsResolutionDto;
}

export interface BootstrapDto {
  environment: EnvironmentDto;
  config: AppConfigDto;
  daemon: DaemonStatusDto;
  launcherPreferences: LauncherPreferencesDto;
  launcherAvailability: LauncherAvailabilityDto;
  domains: DomainInfoDto[];
  projects: ProjectInfoDto[];
  services: ProjectServiceInfoDto[];
  projectEnvironments: ProjectEnvironmentInfoDto[];
  /** Services currently running with a launch profile. */
  launchSessions: LaunchSessionDto[];
  /** The last run of every service Localify started. */
  serviceRuns: ServiceRunDto[];
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
  agent: AgentConfigDto;
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

/** `state` of the daemon's `update_status` (docs/desktop.md, "Automatic updates"). */
export type UpdatePhase =
  | "unknown"
  | "upToDate"
  | "available"
  | "checking"
  | "installing"
  | "failed";

export interface UpdateStatusDto {
  installed: string;
  available: string | null;
  state: UpdatePhase;
  /** Why an available update waits, or why the last check failed. */
  reason: string | null;
  automatic: boolean;
  /** Unix seconds. */
  lastCheckAt: number | null;
  /** False when this copy can only be updated by hand: `reason` says why. */
  canInstall: boolean;
}

export interface UpdateSettingsRequest {
  automatic: boolean;
}

export type DeepLinkRouteDto = "projects" | "logs" | "launchService";

export interface DeepLinkLaunchDto {
  projectId: string;
  environmentSlug: string;
  serviceId: string;
  profile: string | null;
}

export interface DeepLinkPayloadDto {
  route: DeepLinkRouteDto;
  projectId: string | null;
  view: string | null;
  /** The service to start, for `launchService`. */
  launch: DeepLinkLaunchDto | null;
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

export interface RelinkProjectRootRequest {
  projectId: string;
  rootPath: string;
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

export interface RemoveProjectEnvironmentRequest {
  projectId: string;
  environmentSlug: string;
}

export interface RemoveProjectEnvironmentResultDto {
  environment: ProjectEnvironmentInfoDto;
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
  serviceName: string;
  servicePath: string | null;
  command: string[] | null;
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
  serviceName?: string | null;
  servicePath?: string | null;
  command?: string[] | null;
}

export interface InitProjectFromFolderRequest {
  rootPath: string;
  name: string;
  services: InitProjectFromFolderServiceRequest[];
}

export interface InitProjectServiceRequest {
  domain: string;
  target: string;
  serviceName?: string | null;
  servicePath?: string | null;
  command?: string[] | null;
}

export interface AddProjectServiceRequest {
  projectId: string;
  domain: string;
  target: string;
  serviceName?: string | null;
  servicePath?: string | null;
  command?: string[] | null;
}

export interface AddProjectServiceResultDto {
  domain: string;
  target: string;
  daemonReloaded: boolean;
}

export interface OpenInEditorRequest {
  path: string;
}

export interface OpenEnvInEditorRequest {
  projectId: string;
  environmentSlug: string;
  serviceId?: string | null;
  relativePath?: string | null;
}

export interface EnvironmentFileRequest {
  projectId: string;
  environmentSlug: string;
  serviceId?: string | null;
  relativePath?: string | null;
}

export interface EnvironmentFileCandidateDto {
  fileName: string;
  relativePath: string;
  rootLabel: string;
  exists: boolean;
}

export interface EnvironmentFileListDto {
  files: EnvironmentFileCandidateDto[];
}

export interface EnvironmentFileDto {
  fileName: string;
  relativePath: string;
  exists: boolean;
  content: string;
  modifiedAt?: number | null;
}

export interface SaveEnvironmentFileRequest {
  projectId: string;
  environmentSlug: string;
  serviceId?: string | null;
  relativePath?: string | null;
  content: string;
  expectedModifiedAt?: number | null;
}

export interface EnvironmentFileCopyRequest {
  projectId: string;
  sourceEnvironmentSlug: string;
  targetEnvironmentSlug: string;
  relativePaths: string[];
  overwrite: boolean;
}

export type EnvironmentFileCopyStatusDto =
  | "create"
  | "overwrite"
  | "skipExists"
  | "blocked"
  | "copied"
  | "skipped";

export interface EnvironmentFileCopyItemDto {
  relativePath: string;
  fileName: string;
  status: EnvironmentFileCopyStatusDto;
  sourceExists: boolean;
  targetExists: boolean;
  message: string | null;
}

export interface EnvironmentFileCopyPreviewDto {
  items: EnvironmentFileCopyItemDto[];
  createCount: number;
  overwriteCount: number;
  skipCount: number;
  blockedCount: number;
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

/** "unknown" when the daemon is not running. */
export type TunnelStateDto = "stopped" | "starting" | "connected" | "error" | "unknown";

export interface TunnelStatusDto {
  state: TunnelStateDto;
  /** Connections the tunnel holds to Cloudflare. */
  connections: number;
  error: string | null;
  /** What Localify Cloud last told the daemon; null before its first answer. */
  runtime: CloudRuntimeStateDto | null;
  /** Why this Mac may not serve, when `runtime` is "blocked". */
  blocked: CloudRuntimeBlockDto | null;
}

export type CloudRuntimeStateDto = "idle" | "provisioning" | "ready" | "blocked" | "banned";

export interface CloudRuntimeBlockDto {
  reason: "subscription_required" | "serving_slots_full" | "capacity" | (string & {});
  message: string;
  /** What lifts the block, when the account can buy it. */
  upgrade: CloudUpgradeDto | null;
}

/** The command Localify Cloud names to lift a refusal, for a person to run. */
export interface CloudUpgradeDto {
  kind: "pro" | "addon_machine" | "addon_urls" | (string & {});
  command: string;
}

export type CloudPlanStatusDto =
  | "none"
  | "trialing"
  | "active"
  | "past_due"
  | "canceling"
  | "override";

export type CloudPlanIntervalDto = "month" | "year";

export interface CloudPlanCountsDto {
  servingMachines: number;
  publicUrls: number;
}

/** The account's plan, from Localify Cloud. */
export interface CloudPlanDto {
  status: CloudPlanStatusDto;
  entitled: boolean;
  interval: CloudPlanIntervalDto | null;
  /** RFC 3339, while trialing. */
  trialEndsAt: string | null;
  /** RFC 3339: the next charge. */
  renewsAt: string | null;
  /** RFC 3339: when a canceled plan or a team access ends. */
  endsAt: string | null;
  trialAvailable: boolean;
  limits: CloudPlanCountsDto;
  usage: CloudPlanCountsDto;
  /** The add-on units the account holds, counted in `limits`. */
  addons: CloudPlanAddonsDto;
}

export interface CloudPlanAddonsDto {
  machines: number;
  /** Packs of 10 public URLs. */
  urlPacks: number;
}

export interface StartCloudCheckoutRequest {
  interval: CloudPlanIntervalDto;
}

/** "machine": one serving machine per unit; "urls": 10 public URLs per unit. */
export type CloudAddonDto = "machine" | "urls";

export interface StartCloudAddonCheckoutRequest {
  addon: CloudAddonDto;
  /** 1 to 20. */
  units: number;
}

/** The add-ons' monthly prices per unit, in minor units: 300 is $3. */
export interface CloudAddonPricingDto {
  currency: "USD" | "EUR";
  machine: number;
  urlPack: number;
  /** False for the list prices, shown while Localify Cloud cannot answer. */
  live: boolean;
}

/** The page Localify opened in the browser: "portal" when the plan was already active. */
export interface CloudBillingPageDto {
  kind: "checkout" | "portal";
  url: string;
}

export interface CloudAccountDto {
  signedIn: boolean;
  login: string | null;
  machineName: string | null;
  tunnel: TunnelStatusDto;
}

export interface CloudSignInStartDto {
  deviceCode: string;
  userCode: string;
  verificationUri: string;
  /** Seconds to wait between two polls. */
  interval: number;
  /** Seconds before the code expires. */
  expiresIn: number;
}

export interface PollCloudSignInRequest {
  deviceCode: string;
}

/** "denied": the person declined on GitHub. */
export type CloudSignInPollStatusDto =
  | "pending"
  | "slowDown"
  | "signedIn"
  | "denied"
  | "notAllowed"
  | "expired";

export interface CloudSignInPollDto {
  status: CloudSignInPollStatusDto;
  message: string | null;
  /** With "slowDown": the seconds GitHub now asks between polls, when it says. */
  interval: number | null;
  account: CloudAccountDto | null;
}

/** Signing out always forgets the token; `warnings` says what did not follow. */
export interface CloudSignOutDto {
  account: CloudAccountDto;
  warnings: string[];
}

export type PublicUrlProtectionModeDto = "password" | "public";

export interface PublicUrlProtectionDto {
  mode: PublicUrlProtectionModeDto;
}

export interface PublicUrlDto {
  id: string;
  slug: string;
  url: string;
  localDomain: string;
  wildcard: boolean;
  /** The machine that serves it, or null when none does. */
  machineName: string | null;
  servedHere: boolean;
  protection: PublicUrlProtectionDto;
  /** The URL with its current password, which signs a browser in; set when this Mac set that password. */
  passwordLink: string | null;
  /** Null from a Localify Cloud older than the paid plan. */
  state: PublicUrlStateDto | null;
  /** RFC 3339. */
  createdAt: string;
}

/** "suspended": the plan does not cover it right now; "taken_down": removed for abuse. */
export type PublicUrlStateDto = "serving" | "suspended" | "taken_down";

export interface PublicUrlWithPasswordDto {
  publicUrl: PublicUrlDto;
  /** Present only when Localify generated the password. Shown once. */
  password: string | null;
}

/** A password is always generated at creation; it can be changed after. */
export interface CreatePublicUrlRequest {
  localDomain: string;
  wildcard: boolean;
  /** The suggested one when missing. */
  slug?: string;
  mode: PublicUrlProtectionModeDto;
}

export interface SuggestPublicUrlSlugRequest {
  localDomain: string;
}

/** The first free slug that mirrors a local domain's name, and its URL: the public domain comes from it. */
export interface PublicUrlSlugSuggestionDto {
  slug: string;
  url: string;
}

export interface CheckPublicUrlSlugRequest {
  slug: string;
}

export interface PublicUrlSlugCheckDto {
  /** Lower-cased. */
  slug: string;
  available: boolean;
}

export interface PublicUrlRequest {
  id: string;
}

export interface SetPublicUrlProtectionRequest {
  id: string;
  mode: PublicUrlProtectionModeDto;
  password?: string;
}

// --- Remote machines (docs/remote-machines.md) -----------------------------
// The daemon's answers, passed through `DesktopApi` as it sent them.

export type RemoteRelayState = "off" | "connecting" | "connected" | "reconnecting" | "blocked";

/** This machine's switches for paired machines. All start off. */
export interface RemoteSettingsDto {
  share: boolean;
  control: boolean;
  backgroundStarts: boolean;
}

export interface RemoteStatusDto {
  relay: RemoteRelayState;
  blocked: { code: string; message: string } | null;
  /** RFC 3339: the next attempt, while reconnecting. */
  retryAt: string | null;
  settings: RemoteSettingsDto;
  /** This machine's key, as four groups of four hex digits. */
  fingerprint: string | null;
  /** Why background starts run without the user's shell environment. */
  shellEnvironmentError: string | null;
}

export type RemoteMachineState =
  | "offline"
  | "connecting"
  | "reconnecting"
  | "connected"
  | "not_responding"
  | "incompatible"
  | "not_paired"
  | "key_mismatch"
  | "not_sharing"
  | "signed_out"
  | "unavailable";

export interface RemoteMachineDto {
  id: string;
  name: string;
  self: boolean;
  paired: boolean;
  state: RemoteMachineState;
  version: string | null;
  /** From its last `hello`; null until a session said. */
  sharing: boolean | null;
  fingerprint: string | null;
  /** RFC 3339: when the relay saw it come or go. */
  since: string | null;
  /**
   * The session protocol agreed from both `hello`s of the open session to
   * it; null without one. Access requests need 2.
   */
  proto: number | null;
}

export interface RemoteSettingsRequest {
  share?: boolean;
  control?: boolean;
  backgroundStarts?: boolean;
}

export interface RemoteMachineRequest {
  machineId: string;
}

export interface RemotePairAnswerRequest {
  pairingId: string;
  accept: boolean;
}

/** What an access request asks for: `backgroundStarts` asks for `control` too. */
export type RemoteAccessWants = "control" | "backgroundStarts";

/** How an access request ended, as `access.result` says it. */
export type RemoteAccessOutcome = "granted" | "declined" | "nobody" | "expired" | "cancelled";

export type RemoteStreamRequest =
  /** With `access`, the app shows the access dialog; without it, requests get `nobody`. */
  | { kind: "events"; access?: boolean }
  | { kind: "pair"; machineId: string }
  | { kind: "watch"; machineId: string }
  | { kind: "follow"; machineId: string; runId: string; offset: number | null }
  | { kind: "access"; machineId: string; wants: RemoteAccessWants }
  | ({ kind: "control"; machineId: string } & RemoteControlRequest);

/** A control, as the snapshot showed the service when the person clicked. */
export interface RemoteControlRequest {
  action: RemoteControlAction;
  service: { projectId: string; environment: string | null; serviceId: string };
  /** Start only. Null: the effective default profile. */
  profile: string | null;
  expect: { runId: string | null; configRev: string; legacyId?: string };
}

export type RemoteControlOutcome =
  | "done"
  | "starting"
  | "refused"
  | "conflict"
  | "failed"
  | "unknown";

/**
 * What `remote_control` streams: `sent`, its progress, then its result.
 * Without an open session, or when it could not be sent, only a `refused`
 * result. A lost session is `unknown`, followed by the result
 * `control.status` gives, or `lost`; `unknown` is never the last event.
 */
export type RemoteControlEventDto =
  | { event: "sent"; requestId: string }
  | { event: "progress"; phase: "dispatched" | "started" }
  | {
      event: "result";
      outcome: RemoteControlOutcome;
      code: string | null;
      message: string;
      runId: string | null;
    }
  | { event: "unknown"; code: string; message: string }
  | { event: "lost"; code: string; message: string };

/** One line of a stream, or its end. */
export interface RemoteStreamPayload {
  streamId: number;
  data: unknown;
  done: boolean;
  error: string | null;
}

/** What `remote_events` says while the app runs. */
export type RemoteEventDto =
  | { event: "subscribed" }
  | { event: "pair_request"; pairingId: string; machineId: string; name: string; code: string }
  | { event: "paired"; pairingId: string; machineId: string; name: string }
  | { event: "pair_cancelled"; pairingId: string; machineId: string; reason: string }
  /** The pairing ended after a confirm with only one side known to hold a pin. */
  | {
      event: "pair_partial";
      pairingId: string;
      machineId: string;
      name: string;
      local: RemotePinState;
      peer: RemotePinState;
      /** `unconfirmed`: the peer runs a version that cannot say it saved. */
      reason: string;
    }
  | { event: "presence"; machineId: string; state: "online" | "offline" }
  | { event: "revoked"; machineId: string }
  | { event: "unpaired"; machineId: string }
  /** Heard only with `access`. Name and fingerprint come from the local pin. */
  | {
      event: "access_request";
      requestId: string;
      machineId: string;
      name: string;
      fingerprint: string;
      wants: RemoteAccessWants;
      /** RFC 3339. */
      expiresAt: string;
    }
  | { event: "access_closed"; requestId: string };

/** What `remote_access` streams to the machine that asked. */
export type RemoteAccessEventDto =
  | { event: "waiting" }
  | { event: "result"; outcome: RemoteAccessOutcome }
  | { event: "error"; code: string; message: string; retryAfter?: number }
  | { event: "lost"; code: string; message: string };

/** A person's answer in the access dialog, on the target. */
export interface RemoteAccessAnswerRequest {
  requestId: string;
  allow: boolean;
  /** "Don't ask again from this machine". */
  suppress: boolean;
}

export interface RemoteAccessAnswerDto {
  outcome: RemoteAccessOutcome;
}

/** A machine whose access requests are declined at once. */
export interface RemoteAccessSuppressedDto {
  machineId: string;
  name: string;
  /** RFC 3339. */
  since: string;
}

/** A side's pin after a pairing: its own as it verified it, the other's as it said. */
export type RemotePinState = "pinned" | "not_pinned" | "unknown";

/** What `remote_pair` says to the machine that asked. */
export type RemotePairEventDto =
  | { event: "code"; pairingId: string; machineId: string; name: string; code: string }
  | { event: "paired"; machineId: string; name: string }
  | { event: "cancelled"; reason: string; message: string }
  /** After a confirm, only one side may hold a pin (docs/remote-machines.md, "Partial results"). */
  | {
      event: "partial";
      machineId: string;
      name: string;
      local: RemotePinState;
      peer: RemotePinState;
      reason: string;
      message: string;
    };

export interface RemoteMachineSummaryDto {
  machineId: string;
  name: string;
  version: string;
  os: "macos" | "linux";
  settings: RemoteSettingsDto;
  app: "running" | "absent";
}

export type RemoteOwner = "app" | "background" | "terminal" | "unknown";

export type RemoteOutputReason = "terminal" | "unknown_owner" | "no_run" | "legacy_run";

export type RemoteControlAction = "start" | "stop" | "restart";

export interface RemoteInventoryItemDto {
  project: { id: string; name: string };
  environment: { slug: string | null; name: string; primary: boolean; branch: string | null };
  service: { id: string; name: string; configRev: string };
  /** Resolves on the target only. */
  localUrl: string;
  /** Null: nothing runs and the port is free. */
  owner: RemoteOwner | null;
  run: {
    runId: string | null;
    legacy: boolean;
    startedAt: string;
    profile: string | null;
    profileDanger: boolean;
    /** A legacy run whose PID is proven: what a relaunch names. */
    legacyId?: string;
    exit: { at: string; code: number | null } | null;
  } | null;
  process: "running" | "stopped";
  /** `reachable` null: not probed in time. */
  port: { number: number; reachable: boolean | null };
  output: { available: boolean; runId?: string; reason?: RemoteOutputReason };
  publicUrl: {
    url: string;
    serving: boolean;
    reason: "suspended" | "taken_down" | "served_elsewhere" | "tunnel_down" | null;
  } | null;
  controls: Record<RemoteControlAction, { allowed: boolean; code?: string }>;
}

/** Every page of one revision of a target's inventory. */
export interface RemoteInventoryDto {
  machine: RemoteMachineSummaryDto;
  epoch: string;
  rev: number;
  truncated: boolean;
  items: RemoteInventoryItemDto[];
}

/** What `remote_watch` streams. */
export type RemoteWatchEventDto =
  | { event: "connecting" }
  | ({ event: "snapshot" } & RemoteInventoryDto)
  | { event: "lost"; code: string; message: string; retryIn: number };

export interface RemoteOutputLineDto {
  text: string;
  /** The line goes on in the next item. */
  cut?: boolean;
}

export interface RemoteOutputDataDto {
  runId: string;
  offset: number;
  next: number;
  lines: RemoteOutputLineDto[];
  rotated?: { firstOffset: number };
  ended: boolean;
  interrupted?: boolean;
}

/** What `remote_output_follow` streams. */
export type RemoteFollowEventDto =
  | ({ event: "data" } & RemoteOutputDataDto)
  | { event: "ended"; code: string; message: string };
