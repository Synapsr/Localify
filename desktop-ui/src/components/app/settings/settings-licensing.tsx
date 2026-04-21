"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { BookmarkIcon, RefreshIcon, ZapIcon } from "@/components/icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { bootstrapQueries, licenseMutations } from "@/lib/queries";

import { SettingsGroup, SettingsSection } from "./settings-list";

const formatLimit = (value: number | null) => (value === null ? "Unlimited" : String(value));
const formatFingerprint = (value: string) =>
  value.length > 18 ? `${value.slice(0, 8)}…${value.slice(-6)}` : value;

export const SettingsLicensing = () => {
  const queryClient = useQueryClient();
  const { data } = useQuery(bootstrapQueries.read());
  const activateLicense = useMutation(licenseMutations.activate(queryClient));
  const importOfflineLicense = useMutation(
    licenseMutations.importOffline(queryClient),
  );
  const clearLicense = useMutation(licenseMutations.clear(queryClient));
  const refreshLicense = useMutation(licenseMutations.refresh(queryClient));
  const [licenseKey, setLicenseKey] = useState("");
  const [offlineCertificate, setOfflineCertificate] = useState("");
  const [offlineKey, setOfflineKey] = useState("");

  if (!data) return null;

  const { licensing } = data;
  const statusTone: "accent" | "danger" | "muted" =
    licensing.status === "licensed" || licensing.status === "licensedOffline"
      ? "accent"
      : licensing.status === "invalid"
        ? "danger"
        : "muted";
  const mutationError =
    activateLicense.error?.message ??
    importOfflineLicense.error?.message ??
    clearLicense.error?.message ??
    refreshLicense.error?.message ??
    licensing.lastError;

  const activate = async () => {
    await activateLicense.mutateAsync({ key: licenseKey });
    setLicenseKey("");
  };

  const importOffline = async () => {
    await importOfflineLicense.mutateAsync({
      certificate: offlineCertificate,
      key: offlineKey.trim().length > 0 ? offlineKey.trim() : null,
    });
    setOfflineCertificate("");
    setOfflineKey("");
  };

  return (
    <SettingsSection
      label="Licensing"
      icon={<BookmarkIcon />}
      description="Buy a plan, paste the license key here, and Localify handles the rest."
    >
      <SettingsGroup>
        <div className="space-y-4 px-4 py-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="space-y-1">
              <div className="text-sm font-medium text-[--color-ink]">Current plan</div>
              <p className="text-xs text-[--color-muted]">
                {formatLimit(licensing.limits.maxProjects)} projects,{" "}
                {formatLimit(licensing.limits.maxDomainsPerProject)} domains per project
              </p>
            </div>
            <Badge tone={statusTone}>{licensing.limits.plan}</Badge>
          </div>

          {licensing.currentMachine ? (
            <div className="rounded-lg border border-[--color-border]/70 bg-background/70 px-3 py-2 text-xs text-[--color-muted]">
              <div className="font-medium text-[--color-ink]">Current device</div>
              <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
                <span>
                  {licensing.currentMachine.name ??
                    licensing.currentMachine.hostname ??
                    "Unnamed device"}
                </span>
                <span>{licensing.currentMachine.platform}</span>
                <span className="font-mono">
                  {formatFingerprint(licensing.currentMachine.fingerprint)}
                </span>
              </div>
            </div>
          ) : null}

          <div className="rounded-lg border border-[--color-border]/70 bg-background/70 px-3 py-3 text-xs text-[--color-muted]">
            <div className="font-medium text-[--color-ink]">Localify managed licensing</div>
            <p className="mt-1">
              This build already targets the Localify licensing backend. End users only need a
              valid license key.
            </p>
            <div className="mt-3 grid gap-2 sm:grid-cols-2">
              <div className="space-y-1">
                <div className="uppercase tracking-[0.18em]">API</div>
                <div className="font-mono text-[11px] text-[--color-ink]">{licensing.apiUrl}</div>
              </div>
              <div className="space-y-1">
                <div className="uppercase tracking-[0.18em]">Account</div>
                <div className="font-mono text-[11px] text-[--color-ink]">
                  {licensing.account}
                </div>
              </div>
              <div className="space-y-1 sm:col-span-2">
                <div className="uppercase tracking-[0.18em]">Product</div>
                <div className="font-mono text-[11px] text-[--color-ink]">
                  {licensing.product ?? "Not scoped"}
                </div>
              </div>
              <div className="space-y-1 sm:col-span-2">
                <div className="uppercase tracking-[0.18em]">Signature verification</div>
                <div className="text-[--color-ink]">
                  {licensing.publicKey ? "Enabled with embedded Keygen public key" : "Disabled"}
                </div>
              </div>
            </div>
          </div>

          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="ghost"
              loading={refreshLicense.isPending}
              onClick={() => void refreshLicense.mutateAsync()}
              disabled={!licensing.activeLicense}
            >
              <RefreshIcon />
              Refresh license
            </Button>
          </div>
        </div>

        <div className="space-y-4 px-4 py-4">
          <div className="space-y-1">
            <div className="text-sm font-medium text-[--color-ink]">License key</div>
            <p className="text-xs text-[--color-muted]">
              Free includes {licensing.freeLimits.maxProjects} projects and{" "}
              {licensing.freeLimits.maxDomainsPerProject} domains per project.
            </p>
          </div>

          <div className="flex flex-col gap-3 sm:flex-row">
            <Input
              value={licenseKey}
              onChange={(event) => setLicenseKey(event.target.value)}
              placeholder="Paste your Keygen license key"
            />
            <Button
              size="sm"
              loading={activateLicense.isPending}
              onClick={() => void activate()}
              disabled={licenseKey.trim().length === 0}
            >
              <ZapIcon />
              Activate
            </Button>
            <Button
              size="sm"
              variant="outline"
              loading={clearLicense.isPending}
              onClick={() => void clearLicense.mutateAsync()}
              disabled={!licensing.activeLicense}
            >
              Clear
            </Button>
          </div>

          {licensing.activeLicense ? (
            <div className="rounded-lg border border-[--color-border]/70 bg-background/70 px-3 py-2 text-xs text-[--color-muted]">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span>{licensing.activeLicense.keyMasked}</span>
                {licensing.activeLicense.name ? (
                  <span className="text-[--color-ink]">{licensing.activeLicense.name}</span>
                ) : null}
                {licensing.activeLicense.validationCode ? (
                  <Badge tone={statusTone}>{licensing.activeLicense.validationCode}</Badge>
                ) : null}
                {licensing.activeLicense.responseSignatureVerified ? (
                  <Badge tone="accent">Signed response verified</Badge>
                ) : null}
              </div>
              {licensing.activeLicense.machineFingerprint ? (
                <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span>
                    {licensing.activeLicense.machineName ??
                      licensing.activeLicense.machineHostname ??
                      "Activated device"}
                  </span>
                  {licensing.activeLicense.machinePlatform ? (
                    <span>{licensing.activeLicense.machinePlatform}</span>
                  ) : null}
                  <span className="font-mono">
                    {formatFingerprint(licensing.activeLicense.machineFingerprint)}
                  </span>
                  {licensing.activeLicense.machineId ? (
                    <span className="font-mono text-[10px] uppercase tracking-[0.18em]">
                      {licensing.activeLicense.machineId}
                    </span>
                  ) : null}
                </div>
              ) : null}
              {licensing.activeLicense.responseSignatureDate ? (
                <div className="mt-2 text-[10px] uppercase tracking-[0.18em] text-[--color-muted]">
                  Verified at {licensing.activeLicense.responseSignatureDate}
                </div>
              ) : null}
            </div>
          ) : null}

          <div className="rounded-lg border border-[--color-border]/70 bg-background/70 px-3 py-2 text-xs text-[--color-muted]">
            <div className="font-medium text-[--color-ink]">Offline cache</div>
            <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1">
              <span>{licensing.offlineCachePresent ? "Cached" : "Missing"}</span>
              {licensing.offlineLicense?.algorithm ? (
                <span>{licensing.offlineLicense.algorithm}</span>
              ) : null}
              {licensing.offlineLicense?.fileExpiry ? (
                <span>Expires {licensing.offlineLicense.fileExpiry}</span>
              ) : null}
            </div>
          </div>

          <div className="space-y-3">
            <div className="space-y-1">
              <div className="text-sm font-medium text-[--color-ink]">
                Import offline certificate
              </div>
              <p className="text-xs text-[--color-muted]">
                Use this when a machine cannot reach Keygen. Signature verification is already
                embedded in the app.
              </p>
            </div>
            <Textarea
              value={offlineCertificate}
              onChange={(event) => setOfflineCertificate(event.target.value)}
              placeholder="Paste the Keygen offline certificate"
              className="min-h-32"
            />
            <Input
              value={offlineKey}
              onChange={(event) => setOfflineKey(event.target.value)}
              placeholder="Optional override license key"
            />
            <Button
              size="sm"
              variant="outline"
              loading={importOfflineLicense.isPending}
              onClick={() => void importOffline()}
              disabled={!licensing.publicKey}
            >
              Import offline file
            </Button>
          </div>

          <p className="text-xs text-[--color-muted]">
            Activation validates the key, binds this device automatically when required by the
            plan, and refreshes the offline cache when Keygen returns one.
          </p>

          {mutationError ? (
            <p className="text-xs text-[--color-danger]">{mutationError}</p>
          ) : null}
        </div>
      </SettingsGroup>
    </SettingsSection>
  );
};
