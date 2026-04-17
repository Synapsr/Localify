import { mutationOptions, queryOptions } from "@tanstack/react-query";

import type { AvailablePortDto, LogsDto, LogsRequest } from "../../types/roxy";

import { tauriClient } from "../tauri-client";

const logsQueryKeys = {
  all: () => ["logs"] as const,
  byLines: (lines: number) => [...logsQueryKeys.all(), "lines", lines] as const,
} as const;

export const logsQueries = {
  all: logsQueryKeys.all,
  read: (request: LogsRequest) =>
    queryOptions<LogsDto, Error>({
      queryKey: logsQueryKeys.byLines(request.lines),
      queryFn: () => tauriClient.readLogs(request),
    }),
} as const;

export const utilityMutations = {
  availablePort: () =>
    mutationOptions<AvailablePortDto, Error, void>({
      mutationFn: () => tauriClient.getAvailablePort(),
    }),
} as const;
