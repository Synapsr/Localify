import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type {
  AddRouteRequest,
  AssignDomainProjectRequest,
  DomainInfoDto,
  DomainPatternRequest,
  RegisterDomainRequest,
  RemoveRouteRequest,
  UpdateDomainRequest,
} from "../../types/roxy";
import { invalidateBootstrapAnd } from "./shared";
import { projectQueries } from "./project.queries";

const domainQueryKeys = {
  all: () => ["domains"] as const,
  list: () => [...domainQueryKeys.all(), "list"] as const,
} as const;

export const domainQueries = {
  all: domainQueryKeys.all,
  list: () =>
    queryOptions<DomainInfoDto[], Error>({
      queryKey: domainQueryKeys.list(),
      queryFn: () => tauriClient.listDomains(),
    }),
} as const;

const domainMutationInvalidations = [domainQueryKeys.all(), projectQueries.all()] as const;

export const domainMutations = {
  register: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: RegisterDomainRequest) =>
        tauriClient.registerDomain(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
  update: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: UpdateDomainRequest) =>
        tauriClient.updateDomain(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
  assignProject: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: AssignDomainProjectRequest) =>
        tauriClient.assignDomainProject(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
  unregister: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: DomainPatternRequest) =>
        tauriClient.unregisterDomain(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
  addRoute: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: AddRouteRequest) => tauriClient.addRoute(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
  removeRoute: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: RemoveRouteRequest) =>
        tauriClient.removeRoute(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, domainMutationInvalidations),
    }),
} as const;
