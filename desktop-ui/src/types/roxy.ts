export interface EnvironmentDto {
  appName: string;
  desktopName: string;
  configPath: string;
  binaryPath: string;
  binaryExists: boolean;
  fixedDomainSuffix: string;
  cliHelperReady: boolean;
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
}

export interface ProjectInfoDto {
  id: string;
  name: string;
  enabled: boolean;
  domainCount: number;
}

export interface DaemonStatusDto {
  running: boolean;
  pid: number | null;
  lanIp: string;
  caInstalled: boolean;
  httpPort: number;
  httpsPort: number;
  dnsPort: number;
}

export interface BootstrapDto {
  environment: EnvironmentDto;
  config: AppConfigDto;
  daemon: DaemonStatusDto;
  domains: DomainInfoDto[];
  projects: ProjectInfoDto[];
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

export interface CreateProjectRequest {
  name: string;
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
}

export interface AvailablePortDto {
  port: number;
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
