"use client";

import { useState } from "react";

import type { UpdateStatusDto } from "@/types/localify";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

import { SettingsUpdateCard } from "./settings-update-card";

const NOW = 1_780_000_000;

const base: UpdateStatusDto = {
  installed: "1.6.0",
  available: null,
  state: "upToDate",
  reason: null,
  automatic: true,
  lastCheckAt: NOW - 3 * 3600,
  canInstall: true,
};

const previewStates: { label: string; status: UpdateStatusDto }[] = [
  { label: "Never checked", status: { ...base, state: "unknown", lastCheckAt: null } },
  { label: "Up to date", status: base },
  { label: "Checking", status: { ...base, state: "checking" } },
  { label: "Available", status: { ...base, state: "available", available: "1.7.0" } },
  {
    label: "Automatic off",
    status: {
      ...base,
      state: "available",
      available: "1.7.0",
      automatic: false,
      reason:
        "Automatic updates are off. Install it with: localify update install, or turn them on with: localify update --auto on",
    },
  },
  {
    label: "Can't install",
    status: {
      ...base,
      state: "available",
      available: "1.7.0",
      canInstall: false,
      reason:
        "/Applications/Localify.app belongs to another account: only its owner can update it.",
    },
  },
  { label: "Installing", status: { ...base, state: "installing", available: "1.7.0" } },
  {
    label: "Failed",
    status: { ...base, state: "failed", reason: "The update feed could not be reached." },
  },
];

/** Mock states for designing the update card without a daemon. */
export const SettingsUpdatePreview = () => {
  const [selected, setSelected] = useState("Available");
  const preview =
    previewStates.find((previewState) => previewState.label === selected) ?? previewStates[0];

  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h3 className="text-sm font-semibold">Update card preview</h3>
          <p className="text-xs text-[--color-muted]">
            Mock states for designing the updater UI without running a real update.
          </p>
        </div>
        <div className="flex flex-wrap justify-end gap-1.5">
          {previewStates.map((previewState) => (
            <Button
              key={previewState.label}
              size="xs"
              variant={previewState.label === selected ? "secondary" : "ghost"}
              className={cn("rounded-full", previewState.label === selected && "bg-foreground/10")}
              onClick={() => setSelected(previewState.label)}
            >
              {previewState.label}
            </Button>
          ))}
        </div>
      </div>
      <SettingsUpdateCard
        appName="Localify"
        status={preview.status}
        now={NOW}
        error={null}
        automatic={preview.status.automatic}
        automaticDisabled={false}
        onAutomaticChange={() => {}}
        checking={false}
        onCheck={() => setSelected("Checking")}
        restart={
          <Button size="sm" variant="primary" onClick={() => setSelected("Installing")}>
            Restart to update
          </Button>
        }
      />
    </section>
  );
};
