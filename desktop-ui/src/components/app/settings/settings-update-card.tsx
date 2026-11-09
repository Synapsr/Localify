"use client";

import type { ReactNode } from "react";

import type { UpdateStatusDto } from "@/types/localify";

import { DownloadIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { describeUpdate } from "@/lib/util.app-update";

import { SettingsCard } from "./settings-card";

interface SettingsUpdateCardProps {
  appName: string;
  /** Null while it is read. */
  status: UpdateStatusDto | null;
  /** Unix seconds, for the last check's age. */
  now: number;
  error: string | null;
  automatic: boolean;
  automaticDisabled: boolean;
  onAutomaticChange: (automatic: boolean) => void;
  checking: boolean;
  onCheck: () => void;
  /** Shown in place of `Check now` when an update can be installed. */
  restart: ReactNode;
}

export const SettingsUpdateCard = ({
  appName,
  status,
  now,
  error,
  automatic,
  automaticDisabled,
  onAutomaticChange,
  checking,
  onCheck,
  restart,
}: SettingsUpdateCardProps) => {
  const summary = status ? describeUpdate(status, appName, now) : null;
  const busy = status?.state === "checking" || status?.state === "installing";

  return (
    <SettingsCard
      icon={<DownloadIcon />}
      title="Install updates automatically"
      description={`New versions install while ${appName} is closed. While it's open, they wait for you to restart.`}
      trailing={
        <Switch
          aria-label="Install updates automatically"
          checked={automatic}
          disabled={automaticDisabled}
          onCheckedChange={onAutomaticChange}
        />
      }
    >
      <div className="mt-4 flex items-center gap-4 border-t border-border/40 pt-4">
        <div className="min-w-0 flex-1 space-y-0.5">
          <p className="text-sm font-medium text-foreground">
            {summary?.title ?? "Reading the update status…"}
          </p>
          {summary?.detail ? (
            <p className="text-xs leading-normal text-pretty text-muted-foreground">
              {summary.detail}
            </p>
          ) : null}
        </div>
        {summary?.canRestart ? (
          restart
        ) : (
          <Button
            size="sm"
            variant="outline"
            loading={checking}
            disabled={!status || busy}
            onClick={onCheck}
          >
            Check now
          </Button>
        )}
      </div>
      {error ? <p className="pt-3 text-xs leading-normal text-destructive">{error}</p> : null}
    </SettingsCard>
  );
};
