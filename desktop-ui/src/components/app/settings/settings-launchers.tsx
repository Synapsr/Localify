"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { CodeIcon, PlayIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

import type {
  EditorPreference,
  RunActionPreference,
  TerminalPreference,
} from "@/types/roxy";

const runActionOptions = [
  { value: "terminalOnly", label: "Terminal only" },
  { value: "editorAndTerminal", label: "Editor + terminal" },
] satisfies Array<{ value: RunActionPreference; label: string }>;

export const SettingsLaunchers = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const updatePreferences = useMutation(systemMutations.updateLauncherPreferences(queryClient));

  if (!data) return null;

  const preferences = data.launcherPreferences;
  const editorOptions = [
    { value: "auto" as const, label: "Auto" },
    ...data.launcherAvailability.availableEditors.map((option) => ({
      value: option.id,
      label: option.label,
    })),
  ];
  const terminalOptions = data.launcherAvailability.availableTerminals.map((option) => ({
    value: option.id,
    label: option.label,
  }));
  const error = updatePreferences.error instanceof Error ? updatePreferences.error.message : null;

  return (
    <SettingsSection
      label="Launchers"
      icon={<PlayIcon />}
      description="Choose how Localify opens linked projects when you run a local app. Only launchers detected on this Mac are shown."
    >
      <SettingsGroup>
        <SettingsRow
          icon={<CodeIcon />}
          title="Editor"
          description="Used when launch mode is set to open the editor too."
          trailing={
            <div className="flex flex-wrap justify-end gap-1">
              {editorOptions.map((option) => (
                <PreferenceButton
                  key={option.value}
                  active={preferences.editor === option.value}
                  disabled={updatePreferences.isPending}
                  onClick={() =>
                    updatePreferences.mutate({
                      ...preferences,
                      editor: option.value,
                    })
                  }
                >
                  {option.label}
                </PreferenceButton>
              ))}
            </div>
          }
        />
        <SettingsRow
          icon={<TerminalIcon />}
          title="Terminal"
          description="Used for the dev server command launched from project chips and proxy error pages."
          trailing={
            terminalOptions.length > 0 ? (
              <div className="flex flex-wrap justify-end gap-1">
                {terminalOptions.map((option) => (
                  <PreferenceButton
                    key={option.value}
                    active={preferences.terminal === option.value}
                    disabled={updatePreferences.isPending}
                    onClick={() =>
                      updatePreferences.mutate({
                        ...preferences,
                        terminal: option.value,
                      })
                    }
                  >
                    {option.label}
                  </PreferenceButton>
                ))}
              </div>
            ) : (
              <span className="text-xs text-[--color-muted]">No supported terminal detected</span>
            )
          }
        />
        <SettingsRow
          icon={<PlayIcon />}
          title="When running a linked app"
          description="Open only the terminal, or also reopen the project folder in your editor first."
          trailing={
            <div className="flex flex-wrap justify-end gap-1">
              {runActionOptions.map((option) => (
                <PreferenceButton
                  key={option.value}
                  active={preferences.runAction === option.value}
                  disabled={updatePreferences.isPending}
                  onClick={() =>
                    updatePreferences.mutate({
                      ...preferences,
                      runAction: option.value,
                    })
                  }
                >
                  {option.label}
                </PreferenceButton>
              ))}
            </div>
          }
        />
      </SettingsGroup>
      {error ? <p className="px-1 text-xs text-[--color-danger]">{error}</p> : null}
    </SettingsSection>
  );
};

const PreferenceButton = ({
  active,
  disabled,
  onClick,
  children,
}: {
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: string;
}) => {
  return (
    <Button
      type="button"
      size="xs"
      variant={active ? "primary" : "outline"}
      disabled={disabled}
      onClick={onClick}
      className={cn("min-w-20", active && "shadow-none")}
    >
      {children}
    </Button>
  );
};
