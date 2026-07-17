import { mutationOptions, queryOptions, type QueryClient } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type { AppConfigDto, ConfigUpdateRequest } from "../../types/localify";
import { daemonQueries } from "./daemon.queries";
import { invalidateBootstrapAnd } from "./shared";

const configQueryKeys = {
  all: () => ["config"] as const,
  current: () => [...configQueryKeys.all(), "current"] as const,
} as const;

export const configQueries = {
  all: configQueryKeys.all,
  current: () =>
    queryOptions<AppConfigDto, Error>({
      queryKey: configQueryKeys.current(),
      queryFn: () => tauriClient.getConfig(),
    }),
} as const;

export const configMutations = {
  update: (queryClient: QueryClient) =>
    mutationOptions({
      mutationFn: (request: ConfigUpdateRequest) =>
        tauriClient.updateConfig(request),
      onSuccess: async () =>
        invalidateBootstrapAnd(queryClient, [
          configQueryKeys.all(),
          daemonQueries.all(),
        ]),
    }),
} as const;
