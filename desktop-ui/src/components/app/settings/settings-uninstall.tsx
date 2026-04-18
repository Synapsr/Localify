"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { DangerTriangleIcon, TrashIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { bootstrapQueries, systemMutations, systemQueries } from "@/lib/queries";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

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
    <SettingsSection label="Danger zone" icon={<DangerTriangleIcon />}>
      <SettingsGroup className="border-[--color-danger]/20 bg-[--color-danger]/5">
        <SettingsRow
          icon={<TrashIcon />}
          title={`Remove ${appName}`}
          description={`Stops ${appName} and removes it from your Mac, along with your registered domains.`}
          danger
          trailing={
            <Button variant="danger" size="sm" onClick={() => setConfirming(true)}>
              Remove
            </Button>
          }
        />
      </SettingsGroup>

      <Dialog
        open={confirming}
        onOpenChange={(open) => {
          setConfirming(open);
          if (!open) setError(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Remove {appName}?</DialogTitle>
            <DialogDescription>
              {preview.isPending
                ? "Checking what will be removed…"
                : typeof domainCount === "number"
                  ? domainCount === 0
                    ? `This will remove ${appName} from your Mac.`
                    : `This will remove ${appName} and ${domainCount} registered ${domainCount === 1 ? "domain" : "domains"}.`
                  : `This will remove ${appName} from your Mac.`}
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            {preview.error ? (
              <p className="text-xs text-[--color-danger]">{preview.error.message}</p>
            ) : null}
            {error ? <p className="text-xs text-[--color-danger]">{error}</p> : null}
          </DialogBody>
          <DialogFooter>
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
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsSection>
  );
};
