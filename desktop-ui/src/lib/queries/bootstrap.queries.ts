import { queryOptions } from "@tanstack/react-query";

import { tauriClient } from "../tauri-client";
import type { BootstrapDto } from "../../types/roxy";
import { bootstrapQueryKey } from "./shared";

export const bootstrapQueries = {
  all: () => bootstrapQueryKey,
  read: () =>
    queryOptions<BootstrapDto, Error>({
      queryKey: bootstrapQueries.all(),
      queryFn: () => tauriClient.getBootstrap(),
      refetchInterval: 5000,
    }),
} as const;
