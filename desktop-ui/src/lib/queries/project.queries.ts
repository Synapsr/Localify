import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  AddProjectServiceRequest,
  AddProjectServiceResultDto,
  CreateProjectWithDomainsRequest,
  DetectProjectInitPlanDto,
  DetectProjectInitPlanRequest,
  DiscoverProjectWorktreesRequest,
  AttachProjectWorktreeRequest,
  AttachProjectWorktreeResultDto,
  EnvironmentFileCopyPreviewDto,
  EnvironmentFileCopyRequest,
  EnvironmentFileDto,
  EnvironmentFileListDto,
  EnvironmentFileRequest,
  InitProjectFromFolderRequest,
  InitProjectFromFolderResultDto,
  OpenEnvInEditorRequest,
  OpenInEditorRequest,
  ProjectInfoDto,
  ProjectWorktreeDiscoveryDto,
  RemoveProjectEnvironmentRequest,
  RemoveProjectEnvironmentResultDto,
  RemoveProjectRequest,
  RunLocalDevServerRequest,
  SaveEnvironmentFileRequest,
  StopLocalDevServerRequest,
  UpdateProjectRequest,
} from "../../types/roxy";

import { tauriClient } from "../tauri-client";
import { domainQueries } from "./domain.queries";
import { invalidateBootstrapAnd } from "./shared";

const projectQueryKeys = {
  all: () => ["projects"] as const,
  list: () => [...projectQueryKeys.all(), "list"] as const,
  environmentFile: (request: EnvironmentFileRequest) =>
    [
      ...projectQueryKeys.all(),
      "environmentFile",
      request.projectId,
      request.environmentSlug,
      request.serviceId ?? null,
      request.relativePath ?? null,
    ] as const,
  environmentFiles: (request: EnvironmentFileRequest) =>
    [
      ...projectQueryKeys.all(),
      "environmentFiles",
      request.projectId,
      request.environmentSlug,
      request.serviceId ?? null,
    ] as const,
  worktrees: (request: DiscoverProjectWorktreesRequest) =>
    [...projectQueryKeys.all(), "worktrees", request.projectId] as const,
  environmentFileCopyPreview: (request: EnvironmentFileCopyRequest) =>
    [
      ...projectQueryKeys.all(),
      "environmentFileCopyPreview",
      request.projectId,
      request.sourceEnvironmentSlug,
      request.targetEnvironmentSlug,
      request.overwrite,
      ...request.relativePaths,
    ] as const,
} as const;

export const projectQueries = {
  all: projectQueryKeys.all,
  list: () =>
    queryOptions<ProjectInfoDto[], Error>({
      queryKey: projectQueryKeys.list(),
      queryFn: () => tauriClient.listProjects(),
    }),
  environmentFile: (request: EnvironmentFileRequest) =>
    queryOptions<EnvironmentFileDto, Error>({
      queryKey: projectQueryKeys.environmentFile(request),
      queryFn: () => tauriClient.readEnvironmentFile(request),
    }),
  environmentFiles: (request: EnvironmentFileRequest) =>
    queryOptions<EnvironmentFileListDto, Error>({
      queryKey: projectQueryKeys.environmentFiles(request),
      queryFn: () => tauriClient.listEnvironmentFiles(request),
    }),
  worktrees: (request: DiscoverProjectWorktreesRequest) =>
    queryOptions<ProjectWorktreeDiscoveryDto, Error>({
      queryKey: projectQueryKeys.worktrees(request),
      queryFn: () => tauriClient.discoverProjectWorktrees(request),
      retry: false,
    }),
  environmentFileCopyPreview: (request: EnvironmentFileCopyRequest) =>
    queryOptions<EnvironmentFileCopyPreviewDto, Error>({
      queryKey: projectQueryKeys.environmentFileCopyPreview(request),
      queryFn: () => tauriClient.previewEnvironmentFileCopy(request),
    }),
} as const;

export const projectMutations = {
  detectInitPlan: () =>
    mutationOptions<DetectProjectInitPlanDto, Error, DetectProjectInitPlanRequest>({
      mutationFn: (request) => tauriClient.detectProjectInitPlan(request),
    }),
  create: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: CreateProjectWithDomainsRequest) =>
        tauriClient.createProjectWithDomains(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  addService: (queryClient: QueryClient) =>
    mutationOptions<AddProjectServiceResultDto, Error, AddProjectServiceRequest>({
      mutationFn: (request) => tauriClient.addProjectService(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  initFromFolder: (queryClient: QueryClient) =>
    mutationOptions<InitProjectFromFolderResultDto, Error, InitProjectFromFolderRequest>({
      mutationFn: (request) => tauriClient.initProjectFromFolder(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  update: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: UpdateProjectRequest) => tauriClient.updateProject(request),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, [projectQueryKeys.all()]),
    }),
  remove: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: RemoveProjectRequest) => tauriClient.removeProject(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  removeEnvironment: (queryClient: QueryClient) =>
    mutationOptions<RemoveProjectEnvironmentResultDto, Error, RemoveProjectEnvironmentRequest>({
      mutationFn: (request) => tauriClient.removeProjectEnvironment(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  attachWorktree: (queryClient: QueryClient) =>
    mutationOptions<AttachProjectWorktreeResultDto, Error, AttachProjectWorktreeRequest>({
      mutationFn: (request) => tauriClient.attachProjectWorktree(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  openInEditor: () =>
    mutationOptions<void, Error, OpenInEditorRequest>({
      mutationFn: (request) => tauriClient.openInEditor(request),
    }),
  openEnvInEditor: () =>
    mutationOptions<void, Error, OpenEnvInEditorRequest>({
      mutationFn: (request) => tauriClient.openEnvInEditor(request),
    }),
  saveEnvironmentFile: () =>
    mutationOptions<EnvironmentFileDto, Error, SaveEnvironmentFileRequest>({
      mutationFn: (request) => tauriClient.saveEnvironmentFile(request),
    }),
  copyEnvironmentFiles: (queryClient: QueryClient) =>
    mutationOptions<EnvironmentFileCopyPreviewDto, Error, EnvironmentFileCopyRequest>({
      mutationFn: (request) => tauriClient.copyEnvironmentFiles(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  runLocalDevServer: (queryClient: QueryClient) =>
    mutationOptions<void, Error, RunLocalDevServerRequest>({
      mutationFn: (request) => tauriClient.runLocalDevServer(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
  stopLocalDevServer: (queryClient: QueryClient) =>
    mutationOptions<void, Error, StopLocalDevServerRequest>({
      mutationFn: (request) => tauriClient.stopLocalDevServer(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all(), domainQueries.all()]),
    }),
} as const;
