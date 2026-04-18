import { validateHotkey } from "@tanstack/react-hotkeys";
import { z } from "zod";

import { appHotkeyDefinitions, defaultAppHotkeys } from "@/lib/hotkeys";

export const hotkeysSchema = z
  .object(
    Object.fromEntries(
      appHotkeyDefinitions.map((definition) => [
        definition.id,
        z
          .string()
          .trim()
          .min(1, "Shortcut is required")
          .refine((value) => validateHotkey(value).valid, "Invalid shortcut"),
      ]),
    ) as Record<(typeof appHotkeyDefinitions)[number]["id"], z.ZodString>,
  )
  .superRefine((value, ctx) => {
    const seen = new Map<string, (typeof appHotkeyDefinitions)[number]["id"]>();

    for (const definition of appHotkeyDefinitions) {
      const normalized = value[definition.id].trim().toLowerCase();
      const existing = seen.get(normalized);

      if (existing) {
        ctx.addIssue({
          code: "custom",
          path: [definition.id],
          message: `Duplicate shortcut with ${existing}`,
        });
        continue;
      }

      seen.set(normalized, definition.id);
    }
  });

export type HotkeysInput = z.infer<typeof hotkeysSchema>;

export function parseStoredHotkeys(value: unknown): HotkeysInput {
  const candidate =
    typeof value === "object" && value !== null
      ? { ...defaultAppHotkeys, ...value }
      : defaultAppHotkeys;

  const parsed = hotkeysSchema.safeParse(candidate);
  return parsed.success ? parsed.data : defaultAppHotkeys;
}
