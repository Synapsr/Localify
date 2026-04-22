"use client";

import { openUrl } from "@tauri-apps/plugin-opener";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import {
  BookmarkIcon,
  CheckIcon,
  ExternalLinkIcon,
  RefreshIcon,
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
import type { LicensingDto } from "@/types/roxy";

import { SettingsGroup, SettingsSection } from "./settings-list";

const BUY_LICENSE_URL = "https://localify.dev/buy";

const formatLimit = (value: number | null) => (value === null ? "Unlimited" : String(value));

const formatDate = (value: string | null) => {
  if (!value) return null;
  const trimmed = value.trim();
  if (trimmed.length === 0) return null;
  const parsed = /^\d+$/.test(trimmed)
    ? new Date(Number(trimmed) * 1000)
    : new Date(trimmed);
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

  if (!data) return null;

  return (
    <SettingsSection label="License" icon={<BookmarkIcon />}>
      {isActive(data.licensing) ? (
        <LicensedView licensing={data.licensing} />
      ) : (
        <UnlicensedView licensing={data.licensing} />
      )}
    </SettingsSection>
  );
};

const UnlicensedView = ({ licensing }: { licensing: LicensingDto }) => {
  const [dialogOpen, setDialogOpen] = useState(false);

  return (
    <>
      <SettingsGroup>
        <div className="flex flex-col gap-4 px-4 py-4 sm:flex-row sm:items-center sm:justify-between">
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium text-[--color-ink]">
                You&apos;re on the Free plan
              </span>
              <Badge tone="muted">Free</Badge>
            </div>
            <p className="text-xs text-[--color-muted]">
              Up to {formatLimit(licensing.freeLimits.maxProjects)} projects and{" "}
              {formatLimit(licensing.freeLimits.maxDomainsPerProject)} domains per project. Buy a
              license to unlock more, or paste a key you already have.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={() => setDialogOpen(true)}>
              <ZapIcon />
              Enter license key
            </Button>
            <Button size="sm" onClick={() => void openUrl(BUY_LICENSE_URL)}>
              <ExternalLinkIcon />
              Buy a license
            </Button>
          </div>
        </div>
      </SettingsGroup>

      {licensing.lastError ? (
        <p className="px-1 text-xs text-[--color-danger]">{licensing.lastError}</p>
      ) : null}

      <ActivateLicenseDialog open={dialogOpen} onOpenChange={setDialogOpen} />
    </>
  );
};

const LicensedView = ({ licensing }: { licensing: LicensingDto }) => {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const isOffline = licensing.status === "licensedOffline";

  return (
    <>
      <div className="flex items-center gap-3 rounded-xl border border-[--color-accent]/30 bg-[--color-accent]/5 px-4 py-3">
        <span
          aria-hidden
          className="inline-flex size-7 shrink-0 items-center justify-center rounded-full bg-[--color-accent]/15 text-[--color-accent] [&>svg]:size-4"
        >
          <CheckIcon />
        </span>
        <div className="min-w-0 flex-1 space-y-0.5">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm font-medium text-[--color-ink]">
              You have a {licensing.limits.plan} license
            </span>
            {isOffline ? <Badge tone="muted">Offline</Badge> : null}
          </div>
          <p className="text-xs text-[--color-muted]">
            Everything is active. Thanks for supporting Localify.
          </p>
        </div>
        <Button size="sm" variant="outline" onClick={() => setDetailsOpen(true)}>
          Details
        </Button>
      </div>

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
            <InfoItem
              label="Free updates until"
              value={licenseExpiry ?? "Included"}
            />
            {isOffline && offlineExpiry ? (
              <InfoItem label="Offline file expires" value={offlineExpiry} />
            ) : lastValidated ? (
              <InfoItem label="Last checked" value={lastValidated} />
            ) : null}
            {activeLicense?.keyMasked ? (
              <InfoItem
                label="License key"
                value={activeLicense.keyMasked}
                mono
              />
            ) : null}
          </dl>

          {actionError ? (
            <p className="mt-4 text-xs text-[--color-danger]">{actionError}</p>
          ) : null}
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

const InfoItem = ({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) => (
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

  const handleClose = (next: boolean) => {
    if (!next) {
      setLicenseKey("");
      activateLicense.reset();
    }
    onOpenChange(next);
  };

  const submit = async () => {
    await activateLicense.mutateAsync({ key: licenseKey.trim() });
    setLicenseKey("");
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={handleClose}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Enter your license key</DialogTitle>
          <DialogDescription>
            Paste the key you received after purchase. Localify will activate this device
            automatically.
          </DialogDescription>
        </DialogHeader>
        <DialogBody>
          <form
            id="activate-license-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (licenseKey.trim().length > 0) void submit();
            }}
          >
            <Input
              autoFocus
              value={licenseKey}
              onChange={(event) => setLicenseKey(event.target.value)}
              placeholder="Paste your license key"
              aria-invalid={activateLicense.isError || undefined}
            />
            {activateLicense.error ? (
              <p className="mt-2 text-xs text-[--color-danger]">
                {activateLicense.error.message}
              </p>
            ) : null}
          </form>
        </DialogBody>
        <DialogFooter>
          <Button variant="ghost" onClick={() => handleClose(false)}>
            Cancel
          </Button>
          <Button
            type="submit"
            form="activate-license-form"
            loading={activateLicense.isPending}
            disabled={licenseKey.trim().length === 0}
          >
            <ZapIcon />
            Activate
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
