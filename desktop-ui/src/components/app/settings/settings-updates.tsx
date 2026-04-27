"use client";

import { useAppUpdate } from "@/hooks/use-app-update";

import { SettingsUpdateCard } from "./settings-update-card";

export const SettingsUpdates = () => {
  const appUpdate = useAppUpdate();

  return (
    <SettingsUpdateCard
      status={appUpdate.status}
      availableUpdate={appUpdate.availableUpdate}
      errorMessage={appUpdate.errorMessage}
      onCheckForUpdates={() => {
        void appUpdate.checkForUpdates();
      }}
      onInstallUpdate={() => {
        void appUpdate.installUpdate();
      }}
    />
  );
};
