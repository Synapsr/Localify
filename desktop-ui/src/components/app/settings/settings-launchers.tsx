"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { RunActionPreference } from "@/types/roxy";

import { ChevronDownIcon, CodeIcon, PlayIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";

const runActionOptions = [
  { value: "terminalOnly", label: "Terminal only" },
  { value: "editorAndTerminal", label: "Editor + terminal" },
] satisfies Array<{ value: RunActionPreference; label: string }>;

export const SettingsLaunchers = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const updatePreferences = useMutation(systemMutations.updateLauncherPreferences(queryClient));
  const [open, setOpen] = useState(false);

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

  const editorLabel =
    editorOptions.find((option) => option.value === preferences.editor)?.label ?? "Auto";
  const terminalLabel =
    terminalOptions.find((option) => option.value === preferences.terminal)?.label ?? "None";
  const runActionLabel =
    runActionOptions.find((option) => option.value === preferences.runAction)?.label ??
    "Terminal only";
  const summary = [editorLabel, terminalLabel, runActionLabel].join(" · ");

  return (
    <SettingsCard
      icon={<PlayIcon />}
      title="Launchers"
      description={summary}
      action={
        <Button
          size="icon-lg"
          className="rounded-full"
          variant="outline"
          aria-expanded={open}
          aria-label={open ? "Collapse launcher details" : "Expand launcher details"}
          onClick={() => setOpen((current) => !current)}
        >
          <ChevronDownIcon
            className={cn(
              "size-4 transition-transform duration-300 ease-out",
              open && "rotate-180",
            )}
          />
        </Button>
      }
    >
      <div
        className={cn(
          "grid transition-[grid-template-rows] duration-300 ease-out",
          open ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          <div className="mt-5 divide-y divide-border/40 border-t border-border/40 pt-5">
            <LauncherRow
              icon={<CodeIcon />}
              title="Editor"
              description="Used when launch mode reopens the project folder."
            >
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
            </LauncherRow>

            <LauncherRow
              icon={<TerminalIcon />}
              title="Terminal"
              description="Used for dev server commands launched from project chips."
            >
              {terminalOptions.length > 0 ? (
                terminalOptions.map((option) => (
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
                ))
              ) : (
                <span className="text-xs text-muted-foreground">
                  No supported terminal detected
                </span>
              )}
            </LauncherRow>

            <LauncherRow
              icon={<PlayIcon />}
              title="When running a linked app"
              description="Open only the terminal, or also reopen the project folder."
            >
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
            </LauncherRow>
          </div>

          {error ? <p className="px-1 pt-3 text-xs text-[--color-danger]">{error}</p> : null}
        </div>
      </div>
    </SettingsCard>
  );
};

const LauncherRow = ({
  icon,
  title,
  description,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  children: React.ReactNode;
}) => (
  <div className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
    <span
      aria-hidden
      className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-background text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
    >
      {icon}
    </span>
    <div className="min-w-0 flex-1">
      <div className="truncate text-sm font-medium text-foreground">{title}</div>
      <div className="truncate text-xs text-muted-foreground">{description}</div>
    </div>
    <div className="flex flex-wrap justify-end gap-1">{children}</div>
  </div>
);

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
