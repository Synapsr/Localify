import type { RegisterableHotkey } from "@tanstack/react-hotkeys";

export const appHotkeyCategories = [
  { id: "navigation", label: "Navigation" },
  { id: "projects", label: "Projects" },
] as const;

export type AppHotkeyCategory = (typeof appHotkeyCategories)[number]["id"];

export const appHotkeyDefinitions = [
  {
    id: "navigateProjects",
    category: "navigation",
    label: "Go to Projects",
    description: "Open the projects screen.",
    defaultHotkey: "Mod+P",
  },
  {
    id: "navigateLogs",
    category: "navigation",
    label: "Go to Logs",
    description: "Open the logs screen.",
    defaultHotkey: "Mod+L",
  },
  {
    id: "navigateSettings",
    category: "navigation",
    label: "Go to Settings",
    description: "Open the settings screen.",
    defaultHotkey: "Mod+S",
  },
  {
    id: "searchProjects",
    category: "projects",
    label: "Search projects",
    description: "Focus the project search field.",
    defaultHotkey: "Mod+F",
  },
  {
    id: "createProject",
    category: "projects",
    label: "Create project",
    description: "Open the new project dialog.",
    defaultHotkey: "Mod+N",
  },
] as const satisfies readonly {
  id: string;
  category: AppHotkeyCategory;
  label: string;
  description: string;
  defaultHotkey: string;
}[];

export type AppHotkeyId = (typeof appHotkeyDefinitions)[number]["id"];

export type AppHotkeys = Record<AppHotkeyId, string>;

export const defaultAppHotkeys = appHotkeyDefinitions.reduce<AppHotkeys>(
  (accumulator, definition) => {
    accumulator[definition.id] = definition.defaultHotkey;
    return accumulator;
  },
  {} as AppHotkeys,
);

export function toRegisterableHotkey(hotkey: string): RegisterableHotkey {
  return hotkey as RegisterableHotkey;
}
