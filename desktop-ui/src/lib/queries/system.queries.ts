import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
  AgentMcpInstallResultDto,
  AgentMcpStatusDto,
  AutostartStatusDto,
  CliAccessDto,
  InstallResultDto,
  LauncherPreferencesDto,
  NodeTrustDto,
  SettingsExportRequest,
  SettingsExportResultDto,
  SettingsImportPreviewDto,
  SettingsImportPreviewRequest,
  SettingsImportRequest,
  SettingsImportResultDto,
  StepDto,
  UpdateAutostartRequest,
  UpdateSettingsRequest,
  UpdateStatusDto,
  UpdateLauncherPreferencesRequest,
  UninstallPreviewDto,
} from "../../types/localify";

import { tauriClient } from "../tauri-client";
import { configQueries } from "./config.queries";
import { daemonQueries } from "./daemon.queries";
import { domainQueries } from "./domain.queries";
import { projectQueries } from "./project.queries";
import { invalidateBootstrapAnd } from "./shared";

const systemQueryKeys = {
  all: () => ["system"] as const,
  agentMcpStatus: () => [...systemQueryKeys.all(), "agent-mcp-status"] as const,
  autostartStatus: () => [...systemQueryKeys.all(), "autostart-status"] as const,
  uninstallPreview: () => [...systemQueryKeys.all(), "uninstall-preview"] as const,
  updateStatus: () => [...systemQueryKeys.all(), "update-status"] as const,
} as const;

/** The daemon checks every few hours: a slow poll still shows its answer and an install. */
const UPDATE_STATUS_POLL_MS = 5 * 60_000;

const systemMutationInvalidations = [
  daemonQueries.all(),
  domainQueries.all(),
  projectQueries.all(),
  configQueries.all(),
  systemQueryKeys.all(),
] as const;

export const systemQueries = {
  all: systemQueryKeys.all,
  agentMcpStatus: () =>
    queryOptions<AgentMcpStatusDto, Error>({
      queryKey: systemQueryKeys.agentMcpStatus(),
      queryFn: () => tauriClient.getAgentMcpStatus(),
    }),
  autostartStatus: () =>
    queryOptions<AutostartStatusDto, Error>({
      queryKey: systemQueryKeys.autostartStatus(),
      queryFn: () => tauriClient.getAutostartStatus(),
    }),
  uninstallPreview: () =>
    queryOptions<UninstallPreviewDto, Error>({
      queryKey: systemQueryKeys.uninstallPreview(),
      queryFn: () => tauriClient.getUninstallPreview(),
    }),
  updateStatus: () =>
    queryOptions<UpdateStatusDto, Error>({
      queryKey: systemQueryKeys.updateStatus(),
      queryFn: () => tauriClient.getUpdateStatus(),
      refetchInterval: UPDATE_STATUS_POLL_MS,
    }),
} as const;

export const systemMutations = {
  install: (queryClient: QueryClient) =>
    mutationOptions<InstallResultDto, Error, void>({
      mutationFn: () => tauriClient.installLocalify(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  setupCliAccess: (queryClient: QueryClient) =>
    mutationOptions<CliAccessDto, Error, void>({
      mutationFn: () => tauriClient.setupCliAccess(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  setupNodeTrust: (queryClient: QueryClient) =>
    mutationOptions<NodeTrustDto, Error, void>({
      mutationFn: () => tauriClient.setupNodeTrust(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  trustCa: (queryClient: QueryClient) =>
    mutationOptions<void, Error, void>({
      mutationFn: () => tauriClient.trustCa(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, []),
    }),
  trustIosSimulators: () =>
    mutationOptions<StepDto, Error, void>({
      mutationFn: () => tauriClient.trustIosSimulators(),
    }),
  installAgentMcp: (queryClient: QueryClient) =>
    mutationOptions<AgentMcpInstallResultDto, Error, void>({
      mutationFn: () => tauriClient.installAgentMcp(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  exportSettings: () =>
    mutationOptions<SettingsExportResultDto, Error, SettingsExportRequest>({
      mutationFn: (request) => tauriClient.exportSettings(request),
    }),
  previewSettingsImport: () =>
    mutationOptions<SettingsImportPreviewDto, Error, SettingsImportPreviewRequest>({
      mutationFn: (request) => tauriClient.previewSettingsImport(request),
    }),
  importSettings: (queryClient: QueryClient) =>
    mutationOptions<SettingsImportResultDto, Error, SettingsImportRequest>({
      mutationFn: (request) => tauriClient.importSettings(request),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  updateLauncherPreferences: (queryClient: QueryClient) =>
    mutationOptions<LauncherPreferencesDto, Error, UpdateLauncherPreferencesRequest>({
      mutationFn: (request) => tauriClient.updateLauncherPreferences(request),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  updateAutostart: (queryClient: QueryClient) =>
    mutationOptions<AutostartStatusDto, Error, UpdateAutostartRequest>({
      mutationFn: (request) => tauriClient.setAutostartEnabled(request),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  setAutomaticUpdates: (queryClient: QueryClient) =>
    mutationOptions<UpdateStatusDto, Error, UpdateSettingsRequest>({
      mutationFn: (request) => tauriClient.setAutomaticUpdates(request),
      onSuccess: (status) => queryClient.setQueryData(systemQueryKeys.updateStatus(), status),
    }),
  checkForUpdates: (queryClient: QueryClient) =>
    mutationOptions<UpdateStatusDto, Error, void>({
      mutationFn: () => tauriClient.checkForUpdates(),
      onSuccess: (status) => queryClient.setQueryData(systemQueryKeys.updateStatus(), status),
      // A failed check is in the status too.
      onError: async () =>
        queryClient.invalidateQueries({ queryKey: systemQueryKeys.updateStatus() }),
    }),
  restartToUpdate: () =>
    mutationOptions<void, Error, void>({
      mutationFn: () => tauriClient.restartToUpdate(),
    }),
  uninstall: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: () => tauriClient.uninstallLocalify(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
} as const;
