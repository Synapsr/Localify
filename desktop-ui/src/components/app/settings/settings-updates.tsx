"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { RestartToUpdate } from "@/components/app/restart-to-update";
import { bootstrapQueries, systemMutations, systemQueries } from "@/lib/queries";

import { SettingsUpdateCard } from "./settings-update-card";

export const SettingsUpdates = () => {
  const queryClient = useQueryClient();
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const status = useQuery(systemQueries.updateStatus());
  const setAutomatic = useMutation(systemMutations.setAutomaticUpdates(queryClient));
  const check = useMutation(systemMutations.checkForUpdates(queryClient));

  const appName = bootstrap?.environment.appName ?? "Localify";
  const pendingAutomatic = setAutomatic.isPending ? setAutomatic.variables?.automatic : undefined;
  const error = check.error?.message ?? status.error?.message ?? null;

  async function handleAutomaticChange(automatic: boolean) {
    try {
      await setAutomatic.mutateAsync({ automatic });
    } catch (mutationError) {
      const message =
        mutationError instanceof Error ? mutationError.message : String(mutationError);
      toast.error("The update setting didn't change", { description: message });
    }
  }

  return (
    <SettingsUpdateCard
      appName={appName}
      status={status.data ?? null}
      now={Math.floor(Date.now() / 1000)}
      error={error}
      automatic={pendingAutomatic ?? status.data?.automatic ?? false}
      automaticDisabled={!status.data || setAutomatic.isPending}
      onAutomaticChange={(automatic) => void handleAutomaticChange(automatic)}
      checking={check.isPending}
      onCheck={() => check.mutate()}
      restart={
        status.data?.available ? (
          <RestartToUpdate appName={appName} version={status.data.available} />
        ) : null
      }
    />
  );
};
