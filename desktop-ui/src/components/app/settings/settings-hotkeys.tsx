"use client";

import { useEffect, useState } from "react";

import { KeyboardIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  appHotkeyDefinitions,
  defaultAppHotkeys,
  type AppHotkeys,
} from "@/lib/hotkeys";
import { hotkeysSchema } from "@/lib/validators/validator.hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import { SettingsCard } from "./settings-card";
import { SettingsHotkeysRow } from "./settings-hotkeys-row";

import type { AppHotkeyCategory } from "@/lib/hotkeys";

const hotkeyColumns: AppHotkeyCategory[][] = [
  ["navigation"],
  ["appearance", "projects"],
];

export const SettingsHotkeys = () => {
  const { hotkeys, resetHotkeys, setHotkeys } = useAppHotkeys();
  const [open, setOpen] = useState(false);
  const [hotkeysDraft, setHotkeysDraft] = useState<AppHotkeys>(hotkeys);
  const [hotkeysErrorById, setHotkeysErrorById] = useState<Record<string, string>>({});

  useEffect(() => {
    setHotkeysDraft(hotkeys);
    setHotkeysErrorById({});
  }, [hotkeys]);

  const isDirty = appHotkeyDefinitions.some(
    (definition) => hotkeysDraft[definition.id] !== hotkeys[definition.id],
  );

  function saveHotkeys() {
    const parsed = hotkeysSchema.safeParse(hotkeysDraft);

    if (!parsed.success) {
      const nextErrors = parsed.error.issues.reduce<Record<string, string>>(
        (accumulator, issue) => {
          const path = issue.path[0];
          if (typeof path === "string") {
            accumulator[path] = issue.message;
          }
          return accumulator;
        },
        {},
      );
      setHotkeysErrorById(nextErrors);
      return;
    }

    setHotkeysErrorById({});
    setHotkeys(parsed.data);
    setOpen(false);
  }

  function restoreDefaultHotkeys() {
    resetHotkeys();
    setHotkeysDraft(defaultAppHotkeys);
    setHotkeysErrorById({});
  }

  function updateDraftHotkey(id: keyof AppHotkeys, hotkey: string) {
    setHotkeysDraft((current) => ({
      ...current,
      [id]: hotkey,
    }));
    setHotkeysErrorById((current) => {
      const nextErrors = { ...current };
      delete nextErrors[id];
      return nextErrors;
    });
  }

  return (
    <>
      <SettingsCard
        icon={<KeyboardIcon />}
        title="Keyboard shortcuts"
        description="Rebind shortcuts for navigation, appearance, and project actions."
        action={
          <Button size="lg" variant="outline" onClick={() => setOpen(true)}>
            Customize
          </Button>
        }
      />

      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (!next) {
            setHotkeysDraft(hotkeys);
            setHotkeysErrorById({});
          }
          setOpen(next);
        }}
      >
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Keyboard shortcuts</DialogTitle>
            <DialogDescription>
              Click a shortcut to rebind. Esc cancels, Delete clears.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="pb-4">
            <div className="grid gap-4 md:grid-cols-2">
              {hotkeyColumns.map((column, columnIndex) => {
                const definitionsInColumn = appHotkeyDefinitions.filter((definition) =>
                  column.includes(definition.category),
                );
                if (definitionsInColumn.length === 0) return null;
                return (
                  <div
                    key={columnIndex}
                    className="divide-y divide-border/50 overflow-hidden rounded-xl border border-border/60 bg-neutral-50/60 dark:bg-neutral-800/40"
                  >
                    {definitionsInColumn.map((definition) => (
                      <SettingsHotkeysRow
                        key={definition.id}
                        description={definition.description}
                        error={hotkeysErrorById[definition.id]}
                        hotkey={hotkeysDraft[definition.id]}
                        hotkeyId={definition.id}
                        label={definition.label}
                        onChange={updateDraftHotkey}
                      />
                    ))}
                  </div>
                );
              })}
            </div>
          </DialogBody>
          <DialogFooter>
            <Button variant="ghost" size="sm" onClick={restoreDefaultHotkeys}>
              Restore defaults
            </Button>
            <Button variant="primary" size="sm" disabled={!isDirty} onClick={saveHotkeys}>
              Save
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};
