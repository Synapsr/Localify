import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type {
  DaemonStatusDto,
  KillPortConflictRequest,
  PortConflictDto,
  StartDaemonRequest,
} from "../../types/localify";
import { invalidateBootstrapAnd } from "./shared";

const daemonQueryKeys = {
  all: () => ["daemon"] as const,
  status: () => [...daemonQueryKeys.all(), "status"] as const,
  portConflicts: () => [...daemonQueryKeys.all(), "port-conflicts"] as const,
} as const;

export const daemonQueries = {
  all: daemonQueryKeys.all,
  status: () =>
    queryOptions<DaemonStatusDto, Error>({
      queryKey: daemonQueryKeys.status(),
      queryFn: () => tauriClient.getDaemonStatus(),
      refetchInterval: 3000,
    }),
  portConflicts: () =>
    queryOptions<PortConflictDto[], Error>({
      queryKey: daemonQueryKeys.portConflicts(),
      queryFn: () => tauriClient.listPortConflicts(),
    }),
} as const;

export const daemonMutations = {
  start: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: StartDaemonRequest = { verbose: false }) =>
        tauriClient.startDaemon(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [daemonQueryKeys.all()]),
    }),
  stop: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: () => tauriClient.stopDaemon(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [daemonQueryKeys.all()]),
    }),
  restart: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: StartDaemonRequest = { verbose: false }) =>
        tauriClient.restartDaemon(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [daemonQueryKeys.all()]),
    }),
  killPortConflict: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: KillPortConflictRequest) =>
        tauriClient.killPortConflict(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [daemonQueryKeys.all()]),
    }),
} as const;
