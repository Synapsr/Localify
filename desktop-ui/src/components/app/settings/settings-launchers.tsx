"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { BrowserPreference } from "@/types/roxy";

import { ChevronDownIcon, CodeIcon, ExternalLinkIcon, PlayIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";

const browserOptions = [
  { value: "default", label: "Default" },
  { value: "helium", label: "Helium" },
  { value: "arc", label: "Arc" },
  { value: "chrome", label: "Chrome" },
  { value: "safari", label: "Safari" },
  { value: "firefox", label: "Firefox" },
  { value: "brave", label: "Brave" },
] satisfies Array<{ value: BrowserPreference; label: string }>;

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
  const error = updatePreferences.error instanceof Error ? updatePreferences.error.message : null;

  const editorLabel =
    editorOptions.find((option) => option.value === preferences.editor)?.label ?? "Auto";
  const browserLabel =
    browserOptions.find((option) => option.value === preferences.browser)?.label ?? "Default";
  const summary = [editorLabel, browserLabel].join(" · ");

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
              description="Used by Open in editor actions."
            >
              <PreferenceSelect
                value={preferences.editor}
                disabled={updatePreferences.isPending}
                options={editorOptions}
                onChange={(value) => updatePreferences.mutate({ ...preferences, editor: value })}
              />
            </LauncherRow>

            <LauncherRow
              icon={<ExternalLinkIcon />}
              title="Browser"
              description="Used when opening Localify project URLs."
            >
              <PreferenceSelect
                value={preferences.browser}
                disabled={updatePreferences.isPending}
                options={browserOptions}
                onChange={(value) =>
                  updatePreferences.mutate({
                    ...preferences,
                    browser: value as BrowserPreference,
                  })
                }
              />
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

const PreferenceSelect = <T extends string>({
  value,
  options,
  disabled,
  onChange,
}: {
  value: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  disabled?: boolean;
  onChange: (value: T) => void;
}) => {
  return (
    <div className="relative">
      <select
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value as T)}
        className={cn(
          "h-8 min-w-32 appearance-none rounded-md border border-input bg-background py-1 pr-7 pl-2.5 text-sm text-foreground transition-[color,box-shadow] outline-none",
          "hover:border-ring/40 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
          "disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50",
        )}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      <ChevronDownIcon
        aria-hidden
        className="pointer-events-none absolute top-1/2 right-2 size-3.5 -translate-y-1/2 text-muted-foreground"
      />
    </div>
  );
};
