"use client";

import { useQuery } from "@tanstack/react-query";

import { AppStartupScreen } from "@/components/shared/app-startup-screen";
import { bootstrapQueries } from "@/lib/queries";

import { SettingsAgents } from "./settings-agents";
import { SettingsAnalytics } from "./settings-analytics";
import { SettingsCli } from "./settings-cli";
import { SettingsDeveloper } from "./settings-developer";
import { SettingsHeader } from "./settings-header";
import { SettingsHotkeys } from "./settings-hotkeys";
import { SettingsLaunchers } from "./settings-launchers";
import { SettingsLicensing } from "./settings-licensing";
import { SettingsStartup } from "./settings-startup";
import { SettingsStatus } from "./settings-status";
import { SettingsTransfer } from "./settings-transfer";
import { SettingsTroubleshooting } from "./settings-troubleshooting";
import { SettingsUninstall } from "./settings-uninstall";
import { SettingsUpdates } from "./settings-updates";

export const SettingsScreen = () => {
  const { data, error, isPending } = useQuery(bootstrapQueries.read());

  if (isPending && !data) {
    return (
      <AppStartupScreen
        eyebrow="Settings"
        title="Loading settings"
        message="Localify is reading your desktop configuration."
      />
    );
  }

  if (error && !data) {
    return (
      <AppStartupScreen
        eyebrow="Settings"
        title="Cannot load settings"
        message="Localify could not read the desktop configuration."
        error={error.message}
      />
    );
  }

  if (!data) {
    return (
      <AppStartupScreen
        eyebrow="Settings"
        title="Waiting for settings"
        message="The desktop shell is still waiting for configuration data."
      />
    );
  }

  return (
    <div id="settings-screen" className="screen-wrapper space-y-8 pb-12">
      <SettingsHeader />
      <div className="space-y-4">
        <SettingsLicensing />
        <SettingsStatus />
        <SettingsStartup />
        <SettingsTroubleshooting />
        <SettingsLaunchers />
        <SettingsAgents config={data.config} />
        <SettingsAnalytics />
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
