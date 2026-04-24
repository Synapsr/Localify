"use client";

import { useQuery } from "@tanstack/react-query";

import { bootstrapQueries } from "@/lib/queries";

import { SettingsCli } from "./settings-cli";
import { SettingsDeveloper } from "./settings-developer";
import { SettingsHeader } from "./settings-header";
import { SettingsHotkeys } from "./settings-hotkeys";
import { SettingsLaunchers } from "./settings-launchers";
import { SettingsLicensing } from "./settings-licensing";
import { SettingsStatus } from "./settings-status";
import { SettingsTransfer } from "./settings-transfer";
import { SettingsTroubleshooting } from "./settings-troubleshooting";
import { SettingsUninstall } from "./settings-uninstall";
import { SettingsUpdates } from "./settings-updates";

export const SettingsScreen = () => {
  const { data } = useQuery(bootstrapQueries.read());

  if (!data || !data.config.daemon) return null;

  return (
    <div id="settings-screen" className="screen-wrapper space-y-8 pb-12">
      <SettingsHeader />
      <div className="space-y-4">
        <SettingsLicensing />
        <SettingsStatus />
        <SettingsTroubleshooting />
        <SettingsLaunchers />
        <SettingsTransfer />
        <SettingsHotkeys />
        <SettingsCli />
        <SettingsUpdates />
        <SettingsUninstall />
        {import.meta.env.DEV ? <SettingsDeveloper /> : null}
      </div>
    </div>
  );
};
