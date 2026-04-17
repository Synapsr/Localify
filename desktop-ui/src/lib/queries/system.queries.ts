import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type { CliAccessDto, InstallResultDto, UninstallPreviewDto } from "../../types/roxy";
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
  uninstall: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: () => tauriClient.uninstallRoxy(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, systemMutationInvalidations),
    }),
} as const;
