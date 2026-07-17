import { queryOptions } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type { BootstrapDto } from "../../types/localify";
import { bootstrapQueryKey } from "./shared";

export const bootstrapQueries = {
  all: () => bootstrapQueryKey,
  read: () =>
    queryOptions<BootstrapDto, Error>({
      queryKey: bootstrapQueries.all(),
      queryFn: () => tauriClient.getBootstrap(),
      retry: false,
      refetchInterval: (query) => (query.state.data ? 5000 : false),
    }),
} as const;
