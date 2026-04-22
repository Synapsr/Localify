"use client";

import { SettingsLicensingAdvanced } from "./settings-licensing-advanced";
import { SettingsPorts } from "./settings-ports";
import { SettingsProcesses } from "./settings-processes";
import { SettingsSystem } from "./settings-system";

export const SettingsDeveloper = () => {
  return (
    <section className="space-y-4 pt-8">
      <div className="flex items-center gap-2 border-t border-dashed border-[--color-border] pt-6">
        <span className="inline-flex items-center rounded-full bg-amber-500/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-amber-600">
          Dev
        </span>
        <div>
          <h2 className="text-sm font-semibold">Developer tools</h2>
          <p className="text-xs text-[--color-muted]">
            Technical details for debugging. Hidden in production builds.
          </p>
        </div>
      </div>
      <SettingsProcesses />
      <SettingsPorts />
      <SettingsSystem />
      <SettingsLicensingAdvanced />
    </section>
  );
};
