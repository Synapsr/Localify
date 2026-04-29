"use client";

import { motion } from "motion/react";
import { useState } from "react";

import { ActivityIcon, ChevronDownIcon, LockIcon } from "@/components/icons";
import {
  isDesktopAnalyticsEnabled,
  loadDesktopSessionReplayConsent,
  setDesktopSessionReplayConsent,
} from "@/lib/analytics";
import { cn } from "@/lib/utils";

const MotionDiv = motion.create("div");

export const SettingsAnalytics = () => {
  const [sessionReplayEnabled, setSessionReplayEnabled] = useState(loadDesktopSessionReplayConsent);
  const [expanded, setExpanded] = useState(false);
  const analyticsEnabled = isDesktopAnalyticsEnabled();

  const updateSessionReplay = (enabled: boolean) => {
    setSessionReplayEnabled(enabled);
    setDesktopSessionReplayConsent(enabled);
  };

  const panelId = "settings-analytics-panel";

  return (
    <MotionDiv
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", duration: 0.5, bounce: 0 }}
      className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
    >
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={panelId}
        onClick={() => setExpanded((value) => !value)}
        className="flex w-full items-center gap-5 px-6 py-5 text-left transition-colors hover:bg-border/15 focus-visible:bg-border/20 focus-visible:outline-none"
      >
        <span
          aria-hidden
          className="flex size-10 shrink-0 items-center justify-center rounded-full bg-background text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
        >
          <ActivityIcon />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3 className="text-[17px] font-semibold tracking-tight text-foreground">Diagnostics</h3>
          <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
            Share masked session replays when you want help debugging Localify.
          </p>
        </div>
        <ChevronDownIcon
          className={cn(
            "size-4 shrink-0 text-muted-foreground/70 transition-transform duration-300 ease-out",
            expanded && "rotate-180",
          )}
        />
      </button>

      <div
        id={panelId}
        role="region"
        className={cn(
          "grid transition-[grid-template-rows] duration-300 ease-out",
          expanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          <div className="border-t border-border/40 px-6 py-4">
            <div className="flex items-center gap-3">
              <span
                aria-hidden
                className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-background text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
              >
                <LockIcon />
              </span>
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-foreground">Session replay</div>
                <div className="text-xs leading-normal text-muted-foreground">
                  Off by default. When enabled, text and inputs are masked, console logs are
                  skipped, and network timing is disabled.
                </div>
              </div>
              <Toggle
                checked={sessionReplayEnabled}
                disabled={!analyticsEnabled}
                onCheckedChange={updateSessionReplay}
                label="Session replay"
              />
            </div>

            {!analyticsEnabled ? (
              <p className="pt-3 text-xs text-muted-foreground">
                Analytics is not available in this build, so replay cannot be enabled.
              </p>
            ) : null}
          </div>
        </div>
      </div>
    </MotionDiv>
  );
};

interface ToggleProps {
  checked: boolean;
  disabled?: boolean;
  onCheckedChange: (checked: boolean) => void;
  label: string;
}

const Toggle = ({ checked, disabled, onCheckedChange, label }: ToggleProps) => (
  <button
    type="button"
    role="switch"
    aria-checked={checked}
    aria-label={label}
    disabled={disabled}
    onClick={() => onCheckedChange(!checked)}
    className={cn(
      "relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent transition-colors duration-200 outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50",
      checked ? "bg-primary" : "bg-border/70",
    )}
  >
    <span
      aria-hidden
      className={cn(
        "pointer-events-none inline-block size-4 translate-x-0.5 rounded-full bg-background shadow-sm ring-1 ring-black/5 transition-transform duration-200 ease-out",
        checked && "translate-x-[18px]",
      )}
    />
  </button>
);
