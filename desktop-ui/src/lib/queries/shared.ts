import type { QueryClient, QueryKey } from "@tanstack/react-query";

export const bootstrapQueryKey = ["bootstrap"] as const;

export async function invalidateQueries(
  queryClient: QueryClient,
  queryKeys: readonly QueryKey[],
) {
  await Promise.all(
    queryKeys.map((queryKey) => queryClient.invalidateQueries({ queryKey })),
  );
}

export async function invalidateBootstrapAnd(
  queryClient: QueryClient,
  queryKeys: readonly QueryKey[],
) {
  await invalidateQueries(queryClient, [bootstrapQueryKey, ...queryKeys]);
}
