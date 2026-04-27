"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearch } from "@tanstack/react-router";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";

import type { LicensingDto } from "@/types/roxy";

import {
  ArrowRightIcon,
  CheckIcon,
  DangerTriangleIcon,
  KeyIcon,
  RefreshIcon,
  XIcon,
  ZapIcon,
} from "@/components/icons";
import { Badge } from "@/components/ui/badge";
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
import { Input } from "@/components/ui/input";
import { bootstrapQueries, licenseMutations } from "@/lib/queries";

const BUY_LICENSE_URL = "https://localify.dev/#pricing";

const formatLimit = (value: number | null) => (value === null ? "Unlimited" : String(value));

const formatDate = (value: string | null) => {
  if (!value) return null;
  const trimmed = value.trim();
  if (trimmed.length === 0) return null;
  const parsed = /^\d+$/.test(trimmed) ? new Date(Number(trimmed) * 1000) : new Date(trimmed);
  if (Number.isNaN(parsed.getTime())) return trimmed;
  return parsed.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
};

const isActive = (licensing: LicensingDto) =>
  licensing.status === "licensed" || licensing.status === "licensedOffline";

export const SettingsLicensing = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const search = useSearch({ from: "/settings" });

  if (!data) return null;

  const openActivateDialog =
    search.section === "licensing" &&
    search.action === "activateLicense" &&
    !isActive(data.licensing);

  return (
    <section className="space-y-2">
      {isActive(data.licensing) ? (
        <LicensedView licensing={data.licensing} />
      ) : (
        <UnlicensedView licensing={data.licensing} openActivateDialog={openActivateDialog} />
      )}
    </section>
  );
};

const UpsellCard = motion.create("div");

const UnlicensedView = ({
  licensing,
  openActivateDialog,
}: {
  licensing: LicensingDto;
  openActivateDialog: boolean;
}) => {
  const [dialogOpen, setDialogOpen] = useState(false);
  const [dismissedError, setDismissedError] = useState<string | null>(null);
  const visibleError =
    licensing.lastError && licensing.lastError !== dismissedError ? licensing.lastError : null;

  useEffect(() => {
    if (openActivateDialog) {
      setDialogOpen(true);
    }
  }, [openActivateDialog]);

  return (
    <>
      <UpsellCard
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: "spring", duration: 0.5, bounce: 0, delay: 0.05 }}
        className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-6 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
      >
        <div className="flex items-center gap-6">
          <div className="flex min-w-0 flex-1 flex-col gap-2">
            <div className="flex items-baseline gap-2.5">
              <h3 className="text-[17px] font-semibold tracking-tight text-balance text-foreground">
                Upgrade to Pro from <br /> €49/
                <span className="text-muted-foreground tabular-nums">Lifetime</span>
              </h3>
            </div>
            <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
              Unlock unlimited projects and domains. Currently up to{" "}
              {formatLimit(licensing.freeLimits.maxProjects)} projects and{" "}
              {formatLimit(licensing.freeLimits.maxDomainsPerProject)} domains per project.
            </p>
          </div>

          <div className="flex  items-end gap-2">
            <Button size="lg" variant="glow" onClick={() => void openUrl(BUY_LICENSE_URL)}>
              Upgrade
              <ArrowRightIcon data-icon="inline-end" />
            </Button>
            <Button variant="outline" size="lg" type="button" onClick={() => setDialogOpen(true)}>
              I have a license key
            </Button>
          </div>
        </div>
      </UpsellCard>

      <AnimatePresence initial={false}>
        {visibleError ? (
          <motion.div
            key={visibleError}
            initial={{ opacity: 0, y: -4, height: 0 }}
            animate={{ opacity: 1, y: 0, height: "auto" }}
            exit={{ opacity: 0, y: -4, height: 0 }}
            transition={{ type: "spring", duration: 0.35, bounce: 0 }}
            className="overflow-hidden"
          >
            <div
              role="alert"
              className="mt-2 flex items-start gap-2.5 rounded-xl border border-[--color-danger]/20 bg-[--color-danger]/6 px-3 py-2.5 text-[--color-danger]"
            >
              <DangerTriangleIcon
                aria-hidden
                className="mt-px size-3.5 shrink-0 [&>svg]:size-3.5"
              />
              <p className="flex-1 text-xs leading-relaxed">{visibleError}</p>
              <button
                type="button"
                aria-label="Dismiss"
                onClick={() => setDismissedError(visibleError)}
                className="-mt-0.5 -mr-1 flex size-5 shrink-0 items-center justify-center rounded-md text-[--color-danger]/70 transition-colors duration-150 hover:bg-[--color-danger]/10 hover:text-[--color-danger] focus-visible:ring-2 focus-visible:ring-[--color-danger]/40 focus-visible:outline-none"
              >
                <XIcon className="size-3.5" />
              </button>
            </div>
          </motion.div>
        ) : null}
      </AnimatePresence>

      <ActivateLicenseDialog open={dialogOpen} onOpenChange={setDialogOpen} />
    </>
  );
};

const LicensedView = ({ licensing }: { licensing: LicensingDto }) => {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const isOffline = licensing.status === "licensedOffline";

  return (
    <>
      <UpsellCard
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: "spring", duration: 0.5, bounce: 0, delay: 0.05 }}
        className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-5 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
      >
        <div className="flex items-center gap-5">
          <span
            aria-hidden
            className="flex size-10 shrink-0 items-center justify-center rounded-full bg-emerald-500/10 text-emerald-600 dark:text-emerald-500 [&>svg]:size-5"
          >
            <CheckIcon />
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-[17px] font-semibold tracking-tight text-foreground">
                {licensing.limits.plan} license active
              </h3>
              {isOffline ? <Badge tone="muted">Offline</Badge> : null}
            </div>
            <p className="text-[13.5px] leading-normal text-muted-foreground">
              Thanks for supporting Localify — everything is unlocked.
            </p>
          </div>
          <Button size="lg" variant="outline" onClick={() => setDetailsOpen(true)}>
            Details
          </Button>
        </div>
      </UpsellCard>

      <LicenseDetailsDialog
        licensing={licensing}
        open={detailsOpen}
        onOpenChange={setDetailsOpen}
      />
    </>
  );
};

const LicenseDetailsDialog = ({
  licensing,
  open,
  onOpenChange,
}: {
  licensing: LicensingDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) => {
  const queryClient = useQueryClient();
  const refreshLicense = useMutation(licenseMutations.refresh(queryClient));
  const clearLicense = useMutation(licenseMutations.clear(queryClient));

  const { activeLicense, offlineLicense } = licensing;
  const isOffline = licensing.status === "licensedOffline";
  const licenseExpiry =
    formatDate(activeLicense?.expiry ?? null) ?? formatDate(offlineLicense?.licenseExpiry ?? null);
  const offlineExpiry = formatDate(offlineLicense?.fileExpiry ?? null);
  const lastValidated = formatDate(activeLicense?.lastValidatedAt ?? null);

  const actionError =
    refreshLicense.error?.message ?? clearLicense.error?.message ?? licensing.lastError;

  const removeLicense = async () => {
    await clearLicense.mutateAsync();
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>License details</DialogTitle>
          <DialogDescription>
            {licensing.limits.plan} plan — {formatLimit(licensing.limits.maxProjects)} projects,{" "}
            {formatLimit(licensing.limits.maxDomainsPerProject)} domains per project.
          </DialogDescription>
        </DialogHeader>
        <DialogBody className="pb-4">
          <dl className="grid gap-4 text-xs sm:grid-cols-2">
            {activeLicense?.name ? (
              <InfoItem label="Registered to" value={activeLicense.name} />
            ) : null}
            {licensing.currentMachine ? (
              <InfoItem
                label="This device"
                value={
                  licensing.currentMachine.name ??
                  licensing.currentMachine.hostname ??
                  licensing.currentMachine.platform
                }
              />
            ) : null}
            <InfoItem label="Free updates until" value={licenseExpiry ?? "Included"} />
            {isOffline && offlineExpiry ? (
              <InfoItem label="Offline file expires" value={offlineExpiry} />
            ) : lastValidated ? (
              <InfoItem label="Last checked" value={lastValidated} />
            ) : null}
            {activeLicense?.keyMasked ? (
              <InfoItem label="License key" value={activeLicense.keyMasked} mono />
            ) : null}
          </dl>

          {actionError ? <p className="mt-4 text-xs text-[--color-danger]">{actionError}</p> : null}
        </DialogBody>
        <DialogFooter>
          <Button
            variant="ghost"
            loading={clearLicense.isPending}
            onClick={() => void removeLicense()}
          >
            Remove license
          </Button>
          <Button
            variant="outline"
            loading={refreshLicense.isPending}
            onClick={() => void refreshLicense.mutateAsync()}
          >
            <RefreshIcon />
            Refresh
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

const InfoItem = ({ label, value, mono }: { label: string; value: string; mono?: boolean }) => (
  <div className="space-y-0.5">
    <dt className="text-[10px] font-medium tracking-wider text-[--color-muted] uppercase">
      {label}
    </dt>
    <dd className={mono ? "font-mono text-[11px] text-[--color-ink]" : "text-[--color-ink]"}>
      {value}
    </dd>
  </div>
);

const ActivateLicenseDialog = ({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) => {
  const queryClient = useQueryClient();
  const activateLicense = useMutation(licenseMutations.activate(queryClient));
  const [licenseKey, setLicenseKey] = useState("");
  const [succeeded, setSucceeded] = useState(false);

  const handleClose = (next: boolean) => {
    if (!next) {
      setLicenseKey("");
      setSucceeded(false);
      activateLicense.reset();
    }
    onOpenChange(next);
  };

  const submit = async () => {
    await activateLicense.mutateAsync({ key: licenseKey.trim() });
    setSucceeded(true);
    setTimeout(() => {
      setLicenseKey("");
      setSucceeded(false);
      activateLicense.reset();
      onOpenChange(false);
    }, 1400);
  };

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent>
        <AnimatePresence mode="wait" initial={false}>
          {succeeded ? (
            <motion.div
              key="success"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.2 }}
              className="flex flex-col items-center gap-4 px-6 py-10 text-center"
            >
              <motion.span
                aria-hidden
                initial={{ scale: 0.6, opacity: 0 }}
                animate={{ scale: 1, opacity: 1 }}
                transition={{ type: "spring", duration: 0.5, bounce: 0.35 }}
                className="flex size-14 items-center justify-center rounded-full bg-emerald-500/10 text-emerald-600 dark:text-emerald-500 [&>svg]:size-7"
              >
                <CheckIcon />
              </motion.span>
              <div className="space-y-1.5">
                <h2 className="text-[15px] font-medium text-foreground">License activated</h2>
                <p className="text-sm text-muted-foreground">
                  Thanks for supporting Localify — everything is unlocked.
                </p>
              </div>
            </motion.div>
          ) : (
            <motion.div
              key="form"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
            >
              <DialogHeader>
                <DialogTitle>Enter your license key</DialogTitle>
                <DialogDescription>
                  Paste the key you received after purchase. Localify will activate this device
                  automatically.
                </DialogDescription>
              </DialogHeader>
              <DialogBody className="pt-4">
                <form
                  id="activate-license-form"
                  onSubmit={(event) => {
                    event.preventDefault();
                    if (licenseKey.trim().length > 0) void submit();
                  }}
                >
                  <div className="relative">
                    <KeyIcon
                      aria-hidden
                      className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground [&>svg]:size-4"
                    />
                    <Input
                      autoFocus
                      value={licenseKey}
                      onChange={(event) => setLicenseKey(event.target.value)}
                      placeholder="Paste your license key"
                      aria-invalid={activateLicense.isError || undefined}
                      className="h-11 pl-9 font-mono text-[13px]"
                    />
                  </div>
                  <AnimatePresence initial={false}>
                    {activateLicense.error ? (
                      <motion.p
                        initial={{ opacity: 0, y: -4, height: 0 }}
                        animate={{ opacity: 1, y: 0, height: "auto" }}
                        exit={{ opacity: 0, y: -4, height: 0 }}
                        transition={{ type: "spring", duration: 0.3, bounce: 0 }}
                        className="mt-2 text-xs text-[--color-danger]"
                      >
                        {activateLicense.error.message}
                      </motion.p>
                    ) : null}
                  </AnimatePresence>
                  {/* <p className="mt-3 text-xs text-muted-foreground">
                    No key yet?{" "}
                    <button
                      type="button"
                      onClick={() => void openUrl(BUY_LICENSE_URL)}
                      className="font-medium text-foreground underline underline-offset-3 transition-colors hover:text-foreground/80"
                    >
                      Buy a license
                    </button>
                    .
                  </p> */}
                </form>
              </DialogBody>
              <DialogFooter>
                <Button
                  type="submit"
                  form="activate-license-form"
                  size="lg"
                  loading={activateLicense.isPending}
                  disabled={licenseKey.trim().length === 0}
                  className="w-full"
                >
                  <ZapIcon />
                  Activate license
                </Button>
              </DialogFooter>
            </motion.div>
          )}
        </AnimatePresence>
      </DialogContent>
    </Dialog>
  );
};
