"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { PowerIcon } from "@/components/icons";
import { Switch } from "@/components/ui/switch";
import { systemMutations, systemQueries } from "@/lib/queries";

import { SettingsCard } from "./settings-card";

export const SettingsStartup = () => {
  const autostart = useQuery(systemQueries.autostartStatus());
  const queryClient = useQueryClient();
  const updateAutostart = useMutation(systemMutations.updateAutostart(queryClient));

  const pendingEnabled = updateAutostart.variables?.enabled;
  const enabled =
    typeof pendingEnabled === "boolean" ? pendingEnabled : (autostart.data?.enabled ?? false);
  const disabled = autostart.isPending || updateAutostart.isPending || !autostart.data;
  const error = autostart.error instanceof Error ? autostart.error.message : autostart.error;

  async function handleChange(nextEnabled: boolean) {
    try {
      await updateAutostart.mutateAsync({ enabled: nextEnabled });
      toast.success(nextEnabled ? "Launch at login enabled" : "Launch at login disabled");
    } catch (mutationError) {
      const message =
        mutationError instanceof Error ? mutationError.message : String(mutationError);
      toast.error("Launch at login update failed", { description: message });
    }
  }

  return (
    <SettingsCard
      icon={<PowerIcon />}
      title="Startup"
      description="Start Localify in the background and restore the daemon after you sign in."
      trailing={
        <Switch
          aria-label="Launch Localify at login"
          checked={enabled}
          disabled={disabled}
          onCheckedChange={handleChange}
        />
      }
    >
      {error ? <p className="pt-3 text-xs leading-normal text-destructive">{error}</p> : null}
    </SettingsCard>
  );
};
