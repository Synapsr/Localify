import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type {
  ActivateLicenseRequest,
  ImportOfflineLicenseRequest,
  KeygenSettingsUpdateRequest,
  LicensingDto,
} from "../../types/roxy";
import { invalidateBootstrapAnd } from "./shared";

const licenseQueryKeys = {
  all: () => ["licensing"] as const,
  status: () => [...licenseQueryKeys.all(), "status"] as const,
} as const;

export const licenseQueries = {
  all: licenseQueryKeys.all,
  status: () =>
    queryOptions<LicensingDto, Error>({
      queryKey: licenseQueryKeys.status(),
      queryFn: () => tauriClient.getLicensingStatus(),
    }),
} as const;

export const licenseMutations = {
  updateSettings: (queryClient: QueryClient) =>
    mutationOptions<LicensingDto, Error, KeygenSettingsUpdateRequest>({
      mutationFn: (request) => tauriClient.updateKeygenSettings(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [licenseQueryKeys.all()]),
    }),
  activate: (queryClient: QueryClient) =>
    mutationOptions<LicensingDto, Error, ActivateLicenseRequest>({
      mutationFn: (request) => tauriClient.activateLicense(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [licenseQueryKeys.all()]),
    }),
  importOffline: (queryClient: QueryClient) =>
    mutationOptions<LicensingDto, Error, ImportOfflineLicenseRequest>({
      mutationFn: (request) => tauriClient.importOfflineLicense(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [licenseQueryKeys.all()]),
    }),
  clear: (queryClient: QueryClient) =>
    mutationOptions<LicensingDto, Error, void>({
      mutationFn: () => tauriClient.clearLicense(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [licenseQueryKeys.all()]),
    }),
  refresh: (queryClient: QueryClient) =>
    mutationOptions<LicensingDto, Error, void>({
      mutationFn: () => tauriClient.refreshLicense(),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [licenseQueryKeys.all()]),
    }),
} as const;
