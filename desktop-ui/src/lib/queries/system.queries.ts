import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import type {
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
  UpdateAutostartRequest,
  UpdateLauncherPreferencesRequest,
  UninstallPreviewDto,
} from "../../types/roxy";

import { tauriClient } from "../tauri-client";
import { configQueries } from "./config.queries";
import { daemonQueries } from "./daemon.queries";
import { domainQueries } from "./domain.queries";
import { projectQueries } from "./project.queries";
import { invalidateBootstrapAnd } from "./shared";

const systemQueryKeys = {
  all: () => ["system"] as const,
  autostartStatus: () => [...systemQueryKeys.all(), "autostart-status"] as const,
  uninstallPreview: () => [...systemQueryKeys.all(), "uninstall-preview"] as const,
} as const;

const systemMutationInvalidations = [
  daemonQueries.all(),
  domainQueries.all(),
  projectQueries.all(),
  configQueries.all(),
  systemQueryKeys.all(),
] as const;

export const systemQueries = {
  all: systemQueryKeys.all,
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
} as const;

export const systemMutations = {
  install: (queryClient: QueryClient) =>
    mutationOptions<InstallResultDto, Error, void>({
      mutationFn: () => tauriClient.installRoxy(),
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
  uninstall: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: () => tauriClient.uninstallRoxy(),
      onSuccess: async () => invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
} as const;
