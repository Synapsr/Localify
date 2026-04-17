import {
  mutationOptions,
  queryOptions,
  type QueryClient,
} from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";

import type {
  CreateProjectWithDomainsRequest,
  ProjectInfoDto,
  RemoveProjectRequest,
  UpdateProjectRequest,
} from "../../types/roxy";

import { domainQueries } from "./domain.queries";
import { invalidateBootstrapAnd } from "./shared";

const projectQueryKeys = {
  all: () => ["projects"] as const,
  list: () => [...projectQueryKeys.all(), "list"] as const,
} as const;

export const projectQueries = {
  all: projectQueryKeys.all,
  list: () =>
    queryOptions<ProjectInfoDto[], Error>({
      queryKey: projectQueryKeys.list(),
      queryFn: () => tauriClient.listProjects(),
    }),
} as const;

export const projectMutations = {
  create: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: CreateProjectWithDomainsRequest) =>
        tauriClient.createProjectWithDomains(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [
          projectQueryKeys.all(),
          domainQueries.all(),
        ]),
    }),
  update: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: UpdateProjectRequest) =>
        tauriClient.updateProject(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [projectQueryKeys.all()]),
    }),
  remove: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: RemoveProjectRequest) =>
        tauriClient.removeProject(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [
          projectQueryKeys.all(),
          domainQueries.all(),
        ]),
    }),
} as const;
