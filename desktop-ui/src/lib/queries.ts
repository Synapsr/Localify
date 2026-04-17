import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseQueryOptions,
} from "@tanstack/react-query";

import { createDesktopClient } from "./tauri-client";
import { queryKeys } from "./query-keys";
import type {
  AddRouteRequest,
  AssignDomainProjectRequest,
  BootstrapDto,
  ConfigUpdateRequest,
  CreateProjectRequest,
  DaemonStatusDto,
  DomainInfoDto,
  DomainPatternRequest,
  LogsDto,
  LogsRequest,
  ProjectInfoDto,
  RegisterDomainRequest,
  RemoveRouteRequest,
  StartDaemonRequest,
  UpdateProjectRequest,
} from "../types/roxy";

const client = createDesktopClient();

export function useBootstrap(
  options?: Partial<UseQueryOptions<BootstrapDto, Error>>,
) {
  return useQuery<BootstrapDto, Error>({
    queryKey: queryKeys.bootstrap,
    queryFn: () => client.getBootstrap(),
    refetchInterval: 5000,
    ...options,
  });
}

export function useDaemonStatus() {
  return useQuery<DaemonStatusDto, Error>({
    queryKey: queryKeys.daemonStatus,
    queryFn: () => client.getDaemonStatus(),
    refetchInterval: 3000,
  });
}

export function useDomains() {
  return useQuery<DomainInfoDto[], Error>({
    queryKey: queryKeys.domains,
    queryFn: () => client.listDomains(),
  });
}

export function useProjects() {
  return useQuery<ProjectInfoDto[], Error>({
    queryKey: queryKeys.projects,
    queryFn: () => client.listProjects(),
  });
}

export function useConfig() {
  return useQuery({
    queryKey: queryKeys.config,
    queryFn: () => client.getConfig(),
  });
}

export function useLogs(request: LogsRequest, enabled = true) {
  return useQuery<LogsDto, Error>({
    queryKey: [...queryKeys.logs, request.lines],
    queryFn: () => client.readLogs(request),
    enabled,
  });
}

export function useUninstallPreview(enabled = false) {
  return useQuery({
    queryKey: queryKeys.uninstallPreview,
    queryFn: () => client.getUninstallPreview(),
    enabled,
  });
}

function useInvalidateAll() {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: queryKeys.bootstrap });
    qc.invalidateQueries({ queryKey: queryKeys.daemonStatus });
    qc.invalidateQueries({ queryKey: queryKeys.domains });
    qc.invalidateQueries({ queryKey: queryKeys.projects });
    qc.invalidateQueries({ queryKey: queryKeys.config });
  };
}

export function useInstall() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: () => client.installRoxy(),
    onSuccess: invalidate,
  });
}

export function useSetupCliAccess() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: () => client.setupCliAccess(),
    onSuccess: invalidate,
  });
}

export function useUninstall() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: () => client.uninstallRoxy(),
    onSuccess: invalidate,
  });
}

export function useStartDaemon() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: StartDaemonRequest = { verbose: false }) =>
      client.startDaemon(request),
    onSuccess: invalidate,
  });
}

export function useStopDaemon() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: () => client.stopDaemon(),
    onSuccess: invalidate,
  });
}

export function useRestartDaemon() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: StartDaemonRequest = { verbose: false }) =>
      client.restartDaemon(request),
    onSuccess: invalidate,
  });
}

export function useRegisterDomain() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: RegisterDomainRequest) =>
      client.registerDomain(request),
    onSuccess: invalidate,
  });
}

export function useCreateProject() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: CreateProjectRequest) => client.createProject(request),
    onSuccess: invalidate,
  });
}

export function useUpdateProject() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: UpdateProjectRequest) => client.updateProject(request),
    onSuccess: invalidate,
  });
}

export function useAssignDomainProject() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: AssignDomainProjectRequest) =>
      client.assignDomainProject(request),
    onSuccess: invalidate,
  });
}

export function useAvailablePort() {
  return useMutation({
    mutationFn: () => client.getAvailablePort(),
  });
}

export function useUnregisterDomain() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: DomainPatternRequest) =>
      client.unregisterDomain(request),
    onSuccess: invalidate,
  });
}

export function useAddRoute() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: AddRouteRequest) => client.addRoute(request),
    onSuccess: invalidate,
  });
}

export function useRemoveRoute() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: RemoveRouteRequest) => client.removeRoute(request),
    onSuccess: invalidate,
  });
}

export function useUpdateConfig() {
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: (request: ConfigUpdateRequest) => client.updateConfig(request),
    onSuccess: invalidate,
  });
}
