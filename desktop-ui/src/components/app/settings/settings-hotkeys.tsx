"use client";

import { useEffect, useState } from "react";

import { KeyboardIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  appHotkeyDefinitions,
  defaultAppHotkeys,
  type AppHotkeys,
} from "@/lib/hotkeys";
import { hotkeysSchema } from "@/lib/validators/validator.hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import { SettingsHotkeysRow } from "./settings-hotkeys-row";
import { SettingsGroup, SettingsSection } from "./settings-list";

import type { AppHotkeyCategory } from "@/lib/hotkeys";

const hotkeyColumns: AppHotkeyCategory[][] = [
  ["navigation"],
  ["appearance", "projects"],
];

export const SettingsHotkeys = () => {
  const { hotkeys, resetHotkeys, setHotkeys } = useAppHotkeys();
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
    <SettingsSection
      label="Shortcuts"
      icon={<KeyboardIcon />}
      description="Click a shortcut to rebind. Esc cancels, Delete clears."
    >
      <div className="grid gap-4 md:grid-cols-2">
        {hotkeyColumns.map((column, columnIndex) => {
          const definitionsInColumn = appHotkeyDefinitions.filter((definition) =>
            column.includes(definition.category),
          );
          if (definitionsInColumn.length === 0) return null;
          return (
            <SettingsGroup key={columnIndex}>
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
            </SettingsGroup>
          );
        })}
      </div>

      <div className="flex items-center justify-end gap-2 px-1 pt-1">
        <Button variant="ghost" size="sm" onClick={restoreDefaultHotkeys}>
          Restore defaults
        </Button>
        <Button variant="primary" size="sm" disabled={!isDirty} onClick={saveHotkeys}>
          Save
        </Button>
      </div>
    </SettingsSection>
  );
};
