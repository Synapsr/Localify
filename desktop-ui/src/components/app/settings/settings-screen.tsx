"use client";

import { useQuery } from "@tanstack/react-query";

import { bootstrapQueries } from "@/lib/queries";

import { SettingsCli } from "./settings-cli";
import { SettingsDeveloper } from "./settings-developer";
import { SettingsHeader } from "./settings-header";
import { SettingsHotkeys } from "./settings-hotkeys";
import { SettingsServices } from "./settings-services";
import { SettingsStatus } from "./settings-status";
import { SettingsUninstall } from "./settings-uninstall";

export const SettingsScreen = () => {
  const { data } = useQuery(bootstrapQueries.read());

  if (!data || !data.config.daemon) return null;

  return (
    <div id="settings-screen" className="screen-wrapper space-y-6">
      <SettingsHeader />
      <SettingsStatus />
      <SettingsServices />
      <SettingsHotkeys />
      <SettingsCli />
      <SettingsUninstall />
      {import.meta.env.DEV ? <SettingsDeveloper /> : null}
    </div>
  );
};
