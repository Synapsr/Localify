"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { bootstrapQueries, systemMutations, systemQueries } from "@/lib/queries";

export const SettingsUninstall = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const uninstall = useMutation(systemMutations.uninstall(queryClient));
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const preview = useQuery({
    ...systemQueries.uninstallPreview(),
    enabled: confirming,
  });

  const appName = data?.environment.appName ?? "Localify";
  const domainCount = preview.data?.domainCount;

  async function handleUninstall() {
    setError(null);
    try {
      await uninstall.mutateAsync();
      setConfirming(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <Card className="p-5">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="min-w-0 space-y-0.5">
          <p className="text-sm font-medium text-[--color-ink]">Remove {appName}</p>
          <p className="text-xs text-[--color-muted]">
            Stops {appName} and removes it from your Mac, along with your registered domains.
          </p>
        </div>
        {!confirming ? (
          <Button variant="danger" onClick={() => setConfirming(true)}>
            Remove
          </Button>
        ) : null}
      </div>

      {confirming ? (
        <div className="mt-4 space-y-3 rounded-lg border border-[--color-danger]/20 bg-[--color-danger]/5 p-4">
          <div className="space-y-0.5">
            <p className="text-sm font-medium text-[--color-ink]">Are you sure?</p>
            <p className="text-xs text-[--color-muted]">
              {preview.isPending
                ? "Checking what will be removed…"
                : typeof domainCount === "number"
                  ? domainCount === 0
                    ? `This will remove ${appName} from your Mac.`
                    : `This will remove ${appName} and ${domainCount} registered ${domainCount === 1 ? "domain" : "domains"}.`
                  : `This will remove ${appName} from your Mac.`}
            </p>
          </div>

          {preview.error ? (
            <p className="text-xs text-[--color-danger]">{preview.error.message}</p>
          ) : null}
          {error ? <p className="text-xs text-[--color-danger]">{error}</p> : null}

          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setConfirming(false);
                setError(null);
              }}
              disabled={uninstall.isPending}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              size="sm"
              loading={uninstall.isPending}
              onClick={handleUninstall}
            >
              Remove {appName}
            </Button>
          </div>
        </div>
      ) : null}
    </Card>
  );
};
