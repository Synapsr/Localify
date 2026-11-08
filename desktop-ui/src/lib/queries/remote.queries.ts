import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  RemoteMachineDto,
  RemoteMachineRequest,
  RemotePairAnswerRequest,
  RemoteSettingsDto,
  RemoteSettingsRequest,
  RemoteStatusDto,
} from "../../types/localify";

import { tauriClient } from "../tauri-client";
import { invalidateQueries } from "./shared";

/**
 * The daemon answers both from what it holds, without calling Localify Cloud (it reads the
 * roster again at most every 30 seconds), so they can be read often while on screen.
 */
const REMOTE_POLL_MS = 4000;

const remoteQueryKeys = {
  all: () => ["remote"] as const,
  status: () => [...remoteQueryKeys.all(), "status"] as const,
  machines: () => [...remoteQueryKeys.all(), "machines"] as const,
} as const;

export const remoteQueries = {
  all: remoteQueryKeys.all,
  status: () =>
    queryOptions<RemoteStatusDto, Error>({
      queryKey: remoteQueryKeys.status(),
      queryFn: () => tauriClient.getRemoteStatus(),
      refetchInterval: REMOTE_POLL_MS,
      // A daemon from before remote machines answers with an error: no remote UI.
      retry: false,
    }),
  machines: () =>
    queryOptions<RemoteMachineDto[], Error>({
      queryKey: remoteQueryKeys.machines(),
      queryFn: () => tauriClient.getRemoteMachines(),
      refetchInterval: REMOTE_POLL_MS,
      retry: false,
    }),
};

export const remoteMutations = {
  changeSettings: (queryClient: QueryClient) =>
    mutationOptions<RemoteSettingsDto, Error, RemoteSettingsRequest>({
      mutationFn: (request) => tauriClient.changeRemoteSettings(request),
      onSettled: () => invalidateQueries(queryClient, [remoteQueryKeys.all()]),
    }),
  answerPairing: () =>
    mutationOptions<void, Error, RemotePairAnswerRequest>({
      mutationFn: (request) => tauriClient.answerRemotePairing(request),
    }),
  unpair: (queryClient: QueryClient) =>
    mutationOptions<boolean, Error, RemoteMachineRequest>({
      mutationFn: (request) => tauriClient.unpairRemoteMachine(request),
      onSettled: () => invalidateQueries(queryClient, [remoteQueryKeys.all()]),
    }),
  remove: (queryClient: QueryClient) =>
    mutationOptions<void, Error, RemoteMachineRequest>({
      mutationFn: (request) => tauriClient.removeRemoteMachine(request),
      onSettled: () => invalidateQueries(queryClient, [remoteQueryKeys.all()]),
    }),
};
