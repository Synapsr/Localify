"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { BookmarkIcon } from "@/components/icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { bootstrapQueries, licenseMutations } from "@/lib/queries";

import { SettingsGroup, SettingsSection } from "./settings-list";

const formatFingerprint = (value: string) =>
  value.length > 18 ? `${value.slice(0, 8)}…${value.slice(-6)}` : value;

export const SettingsLicensingAdvanced = () => {
  const queryClient = useQueryClient();
  const { data } = useQuery(bootstrapQueries.read());
  const importOfflineLicense = useMutation(licenseMutations.importOffline(queryClient));
  const [offlineCertificate, setOfflineCertificate] = useState("");
  const [offlineKey, setOfflineKey] = useState("");

  if (!data) return null;

  const { licensing } = data;
  const { activeLicense } = licensing;

  const importOffline = async () => {
    await importOfflineLicense.mutateAsync({
      certificate: offlineCertificate,
      key: offlineKey.trim().length > 0 ? offlineKey.trim() : null,
    });
    setOfflineCertificate("");
    setOfflineKey("");
  };

  return (
    <SettingsSection label="Licensing (advanced)" icon={<BookmarkIcon />}>
      <SettingsGroup>
        <div className="space-y-3 px-4 py-4 text-xs text-[--color-muted]">
          <div className="font-medium text-[--color-ink]">Keygen backend</div>
          <div className="grid gap-2 sm:grid-cols-2">
            <InfoBlock label="API" value={licensing.apiUrl} mono />
            <InfoBlock label="Account" value={licensing.account} mono />
            <InfoBlock label="Product" value={licensing.product ?? "Not scoped"} mono />
            <InfoBlock
              label="Signature verification"
              value={
                licensing.publicKey ? "Enabled (embedded public key)" : "Disabled"
              }
            />
          </div>
        </div>

        {licensing.currentMachine ? (
          <div className="space-y-2 px-4 py-4 text-xs text-[--color-muted]">
            <div className="font-medium text-[--color-ink]">This device</div>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
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

        {activeLicense ? (
          <div className="space-y-2 px-4 py-4 text-xs text-[--color-muted]">
            <div className="font-medium text-[--color-ink]">Active license</div>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="font-mono">{activeLicense.keyMasked}</span>
              {activeLicense.validationCode ? (
                <Badge tone="muted">{activeLicense.validationCode}</Badge>
              ) : null}
              {activeLicense.responseSignatureVerified ? (
                <Badge tone="accent">Signed response verified</Badge>
              ) : null}
            </div>
            {activeLicense.machineFingerprint ? (
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="font-mono">
                  {formatFingerprint(activeLicense.machineFingerprint)}
                </span>
                {activeLicense.machineId ? (
                  <span className="font-mono text-[10px] uppercase tracking-[0.18em]">
                    {activeLicense.machineId}
                  </span>
                ) : null}
              </div>
            ) : null}
            {activeLicense.responseSignatureDate ? (
              <div className="text-[10px] uppercase tracking-[0.18em]">
                Verified at {activeLicense.responseSignatureDate}
              </div>
            ) : null}
          </div>
        ) : null}

        <div className="space-y-2 px-4 py-4 text-xs text-[--color-muted]">
          <div className="font-medium text-[--color-ink]">Offline cache</div>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span>{licensing.offlineCachePresent ? "Cached" : "Missing"}</span>
            {licensing.offlineLicense?.algorithm ? (
              <span>{licensing.offlineLicense.algorithm}</span>
            ) : null}
            {licensing.offlineLicense?.fileExpiry ? (
              <span>Expires {licensing.offlineLicense.fileExpiry}</span>
            ) : null}
          </div>
        </div>

        <div className="space-y-3 px-4 py-4">
          <div className="space-y-1">
            <div className="text-sm font-medium text-[--color-ink]">
              Import offline certificate
            </div>
            <p className="text-xs text-[--color-muted]">
              Use when a machine cannot reach Keygen. Signature verification is embedded in the
              app.
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
            disabled={!licensing.publicKey || offlineCertificate.trim().length === 0}
          >
            Import offline file
          </Button>
          {importOfflineLicense.error ? (
            <p className="text-xs text-[--color-danger]">
              {importOfflineLicense.error.message}
            </p>
          ) : null}
        </div>
      </SettingsGroup>
    </SettingsSection>
  );
};

const InfoBlock = ({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) => (
  <div className="space-y-1">
    <div className="text-[10px] font-medium tracking-[0.18em] uppercase">{label}</div>
    <div className={mono ? "font-mono text-[11px] text-[--color-ink]" : "text-[--color-ink]"}>
      {value}
    </div>
  </div>
);
