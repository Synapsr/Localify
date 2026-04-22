import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type {
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
  UpdateLauncherPreferencesRequest,
  UninstallPreviewDto,
} from "../../types/roxy";
import { configQueries } from "./config.queries";
import { daemonQueries } from "./daemon.queries";
import { domainQueries } from "./domain.queries";
import { projectQueries } from "./project.queries";
import { invalidateBootstrapAnd } from "./shared";

const systemQueryKeys = {
  all: () => ["system"] as const,
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
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  setupCliAccess: (queryClient: QueryClient) =>
    mutationOptions<CliAccessDto, Error, void>({
      mutationFn: () => tauriClient.setupCliAccess(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  setupNodeTrust: (queryClient: QueryClient) =>
    mutationOptions<NodeTrustDto, Error, void>({
      mutationFn: () => tauriClient.setupNodeTrust(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
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
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  updateLauncherPreferences: (queryClient: QueryClient) =>
    mutationOptions<LauncherPreferencesDto, Error, UpdateLauncherPreferencesRequest>({
      mutationFn: (request) => tauriClient.updateLauncherPreferences(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
  uninstall: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: () => tauriClient.uninstallRoxy(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
} as const;
