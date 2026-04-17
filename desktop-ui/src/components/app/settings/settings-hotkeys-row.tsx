"use client";

import { formatForDisplay, useHotkeyRecorder } from "@tanstack/react-hotkeys";

import { XIcon } from "@/components/icons";
import { Kbd } from "@/components/ui/kbd";
import type { AppHotkeyId } from "@/lib/hotkeys";
import { cn } from "@/lib/utils";

interface SettingsHotkeysRowProps {
  description: string;
  error?: string;
  hotkey: string;
  hotkeyId: AppHotkeyId;
  label: string;
  onChange: (id: AppHotkeyId, hotkey: string) => void;
}

export const SettingsHotkeysRow = ({
  description,
  error,
  hotkey,
  hotkeyId,
  label,
  onChange,
}: SettingsHotkeysRowProps) => {
  const recorder = useHotkeyRecorder({
    onRecord: (nextHotkey) => onChange(hotkeyId, nextHotkey),
    onClear: () => onChange(hotkeyId, ""),
  });

  const { isRecording, recordedHotkey, startRecording, cancelRecording } = recorder;
  const displayHotkey = recordedHotkey ?? hotkey;

  const handleToggleRecording = () => {
    if (isRecording) {
      cancelRecording();
      return;
    }
    startRecording();
  };

  const handleClear = (event: React.MouseEvent) => {
    event.stopPropagation();
    onChange(hotkeyId, "");
  };

  return (
    <div className="group flex items-center justify-between gap-4 py-2.5">
      <div className="min-w-0">
        <p className="text-sm font-medium text-[--color-ink]">{label}</p>
        <p className="truncate text-xs text-[--color-muted]">{description}</p>
        {error ? (
          <p className="mt-1 text-xs text-[--color-danger]">{error}</p>
        ) : null}
      </div>

      <div className="flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={handleToggleRecording}
          aria-label={
            isRecording
              ? `Stop recording ${label} shortcut`
              : `Record ${label} shortcut`
          }
          className={cn(
            "inline-flex h-7 min-w-24 items-center justify-center rounded-md border px-2 text-xs transition-colors",
            "border-[--color-border] bg-transparent hover:bg-muted/40",
            isRecording &&
              "animate-pulse border-[--color-accent]/60 bg-[--color-accent]/10 hover:bg-[--color-accent]/10",
          )}
        >
          {isRecording ? (
            <span className="text-xs text-[--color-muted]">Press keys…</span>
          ) : displayHotkey ? (
            <Kbd className="bg-transparent text-[--color-ink]">
              {formatForDisplay(displayHotkey)}
            </Kbd>
          ) : (
            <span className="text-xs text-[--color-muted]">Unassigned</span>
          )}
        </button>

        <button
          type="button"
          onClick={handleClear}
          disabled={!displayHotkey || isRecording}
          aria-label={`Clear ${label} shortcut`}
          className={cn(
            "inline-flex size-7 items-center justify-center rounded-md text-[--color-muted] transition-opacity",
            "opacity-0 hover:bg-muted/40 hover:text-[--color-ink]",
            "group-hover:opacity-100 focus-visible:opacity-100",
            "disabled:pointer-events-none disabled:opacity-0",
          )}
        >
          <XIcon className="size-3.5" />
        </button>
      </div>
    </div>
  );
};
