import { useHotkey } from "@tanstack/react-hotkeys";
import { useTheme } from "next-themes";

import { toRegisterableHotkey } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

export const useThemeShortcut = () => {
  const { hotkeys } = useAppHotkeys();
  const { resolvedTheme, setTheme, theme } = useTheme();

  useHotkey(
    toRegisterableHotkey(hotkeys.toggleTheme),
    () => {
      const nextTheme =
        theme === "system"
          ? resolvedTheme === "dark"
            ? "light"
            : "dark"
          : theme === "dark"
            ? "light"
            : "dark";

      setTheme(nextTheme);
    },
    { preventDefault: true },
  );
};
