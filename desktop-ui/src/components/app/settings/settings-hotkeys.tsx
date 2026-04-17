"use client";

import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  appHotkeyCategories,
  appHotkeyDefinitions,
  defaultAppHotkeys,
  type AppHotkeys,
} from "@/lib/hotkeys";
import { hotkeysSchema } from "@/lib/validators/validator.hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import { SettingsHotkeysRow } from "./settings-hotkeys-row";

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
    <Card className="p-5">
      <div className="space-y-4">
        <div className="space-y-1">
          <h2 className="text-sm font-semibold">Shortcuts</h2>
          <p className="text-xs text-[--color-muted]">
            Click a shortcut to rebind. Esc cancels, Delete clears.
          </p>
        </div>

        <div className="grid gap-6 md:grid-cols-2">
          {appHotkeyCategories.map((category) => {
            const definitionsInCategory = appHotkeyDefinitions.filter(
              (definition) => definition.category === category.id,
            );
            if (definitionsInCategory.length === 0) return null;
            return (
              <div key={category.id} className="space-y-1">
                <h3 className="text-[11px] font-medium uppercase tracking-wide text-[--color-muted]">
                  {category.label}
                </h3>
                <ul className="divide-y divide-[--color-border]">
                  {definitionsInCategory.map((definition) => (
                    <li key={definition.id}>
                      <SettingsHotkeysRow
                        description={definition.description}
                        error={hotkeysErrorById[definition.id]}
                        hotkey={hotkeysDraft[definition.id]}
                        hotkeyId={definition.id}
                        label={definition.label}
                        onChange={updateDraftHotkey}
                      />
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
        </div>

        <div className="flex items-center justify-between gap-2 border-t border-[--color-border] pt-4">
          <Button variant="ghost" size="sm" onClick={restoreDefaultHotkeys}>
            Restore defaults
          </Button>
          <Button variant="primary" size="sm" disabled={!isDirty} onClick={saveHotkeys}>
            Save
          </Button>
        </div>
      </div>
    </Card>
  );
};
