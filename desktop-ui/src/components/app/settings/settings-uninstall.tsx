"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";

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
import { clearStoredOnboardingCompleted } from "@/lib/util.onboarding-state";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

function isUserCancelledNativePrompt(error: unknown): boolean {
  if (!(error instanceof Error)) {
    return false;
  }

  const message = error.message.trim().toLowerCase();
  return (
    message.includes("user canceled") ||
    message.includes("user cancelled") ||
    message.includes("execution error: user canceled") ||
    message.includes("execution error: user cancelled") ||
    message.includes("(-128)")
  );
}

const waitForNextPaint = () =>
  new Promise<void>((resolve) => {
    requestAnimationFrame(() => resolve());
  });

export const SettingsUninstall = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const uninstall = useMutation(systemMutations.uninstall(queryClient));
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const preview = useQuery({
    ...systemQueries.uninstallPreview(),
    enabled: confirming,
  });

  const appName = data?.environment.appName ?? "Localify";
  const domainCount = preview.data?.domainCount;
  const previewDataDir = preview.data?.dataDir;
  const isWorking = isSubmitting || uninstall.isPending;

  async function handleUninstall() {
    setError(null);
    setIsSubmitting(true);

    try {
      await waitForNextPaint();
      await uninstall.mutateAsync();
      clearStoredOnboardingCompleted();
      window.location.reload();
    } catch (err) {
      if (isUserCancelledNativePrompt(err)) {
        setConfirming(false);
        setError(null);
        setIsSubmitting(false);
        return;
      }

      setError(err instanceof Error ? err.message : String(err));
      setIsSubmitting(false);
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
            <Button
              variant="danger"
              size="sm"
              onClick={() => setConfirming(true)}
              disabled={confirming}
            >
              Remove
            </Button>
          }
        />
      </SettingsGroup>

      <Dialog
        open={confirming}
        onOpenChange={(open) => {
          if (isWorking) {
            return;
          }

          setConfirming(open);
          if (!open) setError(null);
        }}
      >
        <DialogContent showCloseButton={!isWorking} className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle className="text-balance">
              {isWorking ? `Removing ${appName}…` : `Remove ${appName}?`}
            </DialogTitle>
            <DialogDescription className="text-pretty">
              {isWorking
                ? `${appName} is being removed from this Mac. The window will refresh as soon as cleanup finishes.`
                : preview.isPending
                  ? "Checking what will be removed…"
                  : typeof domainCount === "number"
                    ? domainCount === 0
                      ? `This will remove ${appName} from your Mac.`
                      : `This will remove ${appName} and ${domainCount} registered ${domainCount === 1 ? "domain" : "domains"}.`
                    : `This will remove ${appName} from your Mac.`}
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-4 pb-2">
            <div className="rounded-2xl bg-[--color-danger]/6 p-4 shadow-[inset_0_0_0_1px_rgb(239_68_68/0.14),0_12px_30px_rgb(15_23_42/0.04)]">
              <div className="flex items-start gap-3">
                <div className="mt-0.5 flex size-10 shrink-0 items-center justify-center rounded-xl bg-[--color-danger]/10 text-[--color-danger] shadow-[inset_0_0_0_1px_rgb(239_68_68/0.12)]">
                  <TrashIcon className="size-4" />
                </div>
                <div className="min-w-0 space-y-2">
                  <p className="text-sm font-medium text-[--color-ink] text-pretty">
                    {isWorking
                      ? "Removing the daemon, local trust configuration, and Localify-managed files."
                      : "This action removes Localify-managed system setup and local routing data from this Mac."}
                  </p>
                  {typeof domainCount === "number" ? (
                    <p className="text-xs text-[--color-muted]">
                      Registered domains to remove:{" "}
                      <span className="font-medium tabular-nums text-[--color-ink]">
                        {domainCount}
                      </span>
                    </p>
                  ) : null}
                  {previewDataDir ? (
                    <p className="truncate text-xs text-[--color-muted]">
                      Local data folder: <span className="font-mono">{previewDataDir}</span>
                    </p>
                  ) : null}
                </div>
              </div>
            </div>

            <AnimatePresence initial={false} mode="wait">
              {isWorking ? (
                <motion.div
                  key="working"
                  initial={{ opacity: 0, y: 8, filter: "blur(4px)" }}
                  animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                  exit={{ opacity: 0, y: 6, filter: "blur(4px)" }}
                  transition={{ type: "spring", duration: 0.3, bounce: 0 }}
                  className="rounded-xl bg-[--color-border]/30 px-4 py-3 text-xs text-[--color-muted] shadow-[inset_0_0_0_1px_rgb(15_23_42/0.05)]"
                >
                  macOS may ask once for your administrator password. Keep this window open and
                  Localify will stay on the in-app loading state until cleanup finishes.
                </motion.div>
              ) : null}
            </AnimatePresence>

            {preview.error ? (
              <p className="rounded-xl bg-[--color-danger]/6 px-3 py-2 text-xs text-[--color-danger] shadow-[inset_0_0_0_1px_rgb(239_68_68/0.12)]">
                {preview.error.message}
              </p>
            ) : null}
            {error ? (
              <p className="rounded-xl bg-[--color-danger]/6 px-3 py-2 text-xs text-[--color-danger] shadow-[inset_0_0_0_1px_rgb(239_68_68/0.12)]">
                {error}
              </p>
            ) : null}
          </DialogBody>

          <DialogFooter className="pt-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                setConfirming(false);
                setError(null);
              }}
              disabled={isWorking}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              size="sm"
              loading={isWorking}
              onClick={handleUninstall}
            >
              {isWorking ? `Removing ${appName}…` : `Remove ${appName}`}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsSection>
  );
};
