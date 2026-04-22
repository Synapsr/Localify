import { z } from "zod";

export const launcherPreferencesSchema = z.object({
  editor: z.enum(["auto", "cursor", "vsCode", "windsurf", "zed", "sublimeText", "nova"]),
  terminal: z.enum(["terminal", "ghostty", "iTerm", "wezTerm"]),
  runAction: z.enum(["terminalOnly", "editorAndTerminal"]),
});

export type LauncherPreferencesInput = z.infer<typeof launcherPreferencesSchema>;
