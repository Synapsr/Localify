"use client";

import { motion } from "motion/react";

import { DownloadIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import type { AppUpdateStatus } from "@/providers/app-update-provider";
import type { AppUpdateInfoDto } from "@/types/localify";

interface SettingsUpdateCardProps {
  status: AppUpdateStatus;
  availableUpdate: AppUpdateInfoDto | null;
  errorMessage: string | null;
  onCheckForUpdates: () => void;
  onInstallUpdate: () => void;
}

export const SettingsUpdateCard = ({
  status,
  availableUpdate,
  errorMessage,
  onCheckForUpdates,
  onInstallUpdate,
}: SettingsUpdateCardProps) => {
  const isChecking = status === "checking";
  const isInstalling = status === "installing";
  const hasUpdate = Boolean(availableUpdate);
  const title = hasUpdate
    ? `Update ready - v${availableUpdate?.version}`
    : status === "upToDate"
      ? "You're up to date"
      : status === "unconfigured"
        ? "Updates unavailable"
        : "Desktop updates";
  const description = hasUpdate
    ? "A new Localify build is ready to download and install."
    : status === "unconfigured"
      ? "Auto-updates are not configured for this build."
      : "Checks for new Localify builds in the background and on demand.";

  const action = hasUpdate ? (
    <Button
      size="lg"
      variant="primary"
      loading={isInstalling}
      disabled={isChecking}
      onClick={onInstallUpdate}
    >
      Install
    </Button>
  ) : (
    <Button
      size="lg"
      variant="outline"
      loading={isChecking}
      disabled={isInstalling || status === "unconfigured"}
      onClick={onCheckForUpdates}
    >
      Check now
    </Button>
  );

  return (
    <section className="space-y-2">
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: "spring", duration: 0.5, bounce: 0, delay: 0.1 }}
        className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-5 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
      >
        <div className="flex items-center gap-5">
          <span
            aria-hidden
            className="flex size-10 shrink-0 items-center justify-center rounded-full bg-background text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
          >
            <DownloadIcon />
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <h3 className="text-[17px] font-semibold tracking-tight text-foreground">
              {title}
            </h3>
            <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
              {description}
            </p>
          </div>
          {action}
        </div>
      </motion.div>

      {errorMessage ? (
        <p className="px-1 text-xs text-[--color-danger]">{errorMessage}</p>
      ) : null}
    </section>
  );
};
