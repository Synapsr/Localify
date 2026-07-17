"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { AppUpdateStatus } from "@/providers/app-update-provider";
import type { AppUpdateInfoDto } from "@/types/localify";

import { SettingsUpdateCard } from "./settings-update-card";

type UpdatePreviewState = {
  status: AppUpdateStatus;
  label: string;
  update: AppUpdateInfoDto | null;
  errorMessage: string | null;
};

const previewUpdate = {
  version: "1.4.0",
  notes: "Mock release notes for the update card preview.",
  pubDate: "2026-04-27T10:00:00Z",
} satisfies AppUpdateInfoDto;

const previewStates = [
  {
    status: "idle",
    label: "Idle",
    update: null,
    errorMessage: null,
  },
  {
    status: "checking",
    label: "Checking",
    update: null,
    errorMessage: null,
  },
  {
    status: "available",
    label: "Available",
    update: previewUpdate,
    errorMessage: null,
  },
  {
    status: "installing",
    label: "Installing",
    update: previewUpdate,
    errorMessage: null,
  },
  {
    status: "upToDate",
    label: "Up to date",
    update: null,
    errorMessage: null,
  },
  {
    status: "unconfigured",
    label: "Unconfigured",
    update: null,
    errorMessage: null,
  },
  {
    status: "error",
    label: "Error",
    update: previewUpdate,
    errorMessage: "Localify could not download the latest desktop update.",
  },
] satisfies UpdatePreviewState[];

export const SettingsUpdatePreview = () => {
  const [selectedStatus, setSelectedStatus] = useState<AppUpdateStatus>("checking");
  const selectedState =
    previewStates.find((previewState) => previewState.status === selectedStatus) ??
    previewStates[0];

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
              key={previewState.status}
              size="xs"
              variant={previewState.status === selectedStatus ? "secondary" : "ghost"}
              className={cn(
                "rounded-full",
                previewState.status === selectedStatus && "bg-foreground/10",
              )}
              onClick={() => {
                setSelectedStatus(previewState.status);
              }}
            >
              {previewState.label}
            </Button>
          ))}
        </div>
      </div>
      <SettingsUpdateCard
        status={selectedState.status}
        availableUpdate={selectedState.update}
        errorMessage={selectedState.errorMessage}
        onCheckForUpdates={() => {
          setSelectedStatus("checking");
        }}
        onInstallUpdate={() => {
          setSelectedStatus("installing");
        }}
      />
    </section>
  );
};
