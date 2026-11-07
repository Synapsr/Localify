import type { RegisterableHotkey } from "@tanstack/react-hotkeys";

export const appHotkeyCategories = [
  { id: "navigation", label: "Navigation" },
  { id: "appearance", label: "Appearance" },
  { id: "projects", label: "Projects" },
  { id: "terminal", label: "Terminal" },
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
    id: "toggleTheme",
    category: "appearance",
    label: "Toggle theme",
    description: "Switch between the light and dark app themes.",
    defaultHotkey: "Mod+Alt+Shift+T",
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
  {
    id: "toggleTerminal",
    category: "terminal",
    label: "Toggle terminal",
    description: "Show or hide the integrated terminal.",
    defaultHotkey: "Mod+J",
  },
  {
    id: "newTerminalTab",
    category: "terminal",
    label: "New terminal tab",
    description: "Open a new shell in the current checkout.",
    defaultHotkey: "Mod+T",
  },
  {
    id: "closeTerminalTab",
    category: "terminal",
    label: "Close terminal tab",
    description: "Close the active terminal tab.",
    defaultHotkey: "Mod+W",
  },
  {
    id: "previousTerminalTab",
    category: "terminal",
    label: "Previous terminal tab",
    description: "Select the previous tab of the current checkout.",
    defaultHotkey: "Mod+Shift+[",
  },
  {
    id: "nextTerminalTab",
    category: "terminal",
    label: "Next terminal tab",
    description: "Select the next tab of the current checkout.",
    defaultHotkey: "Mod+Shift+]",
  },
  {
    id: "previousTerminalEnvironment",
    category: "terminal",
    label: "Previous running checkout",
    description: "Select the previous checkout in the terminal sidebar.",
    defaultHotkey: "Mod+Alt+[",
  },
  {
    id: "nextTerminalEnvironment",
    category: "terminal",
    label: "Next running checkout",
    description: "Select the next checkout in the terminal sidebar.",
    defaultHotkey: "Mod+Alt+]",
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
