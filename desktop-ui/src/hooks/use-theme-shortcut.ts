import { isTauri } from "@tauri-apps/api/core";
import {
  register as registerGlobalShortcut,
  unregister as unregisterGlobalShortcut,
} from "@tauri-apps/plugin-global-shortcut";
import { useTheme } from "next-themes";
import { useEffect, useEffectEvent } from "react";

import { toGlobalShortcut } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

export const useThemeShortcut = () => {
  const { hotkeys } = useAppHotkeys();
  const { resolvedTheme, setTheme, theme } = useTheme();
  const globalShortcut = toGlobalShortcut(hotkeys.toggleTheme);

  const handleShortcutPressed = useEffectEvent(() => {
    const nextTheme =
      theme === "system"
        ? resolvedTheme === "dark"
          ? "light"
          : "dark"
        : theme === "dark"
          ? "light"
          : "dark";

    setTheme(nextTheme);
  });

  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    let disposed = false;

    void (async () => {
      await unregisterGlobalShortcut(globalShortcut).catch(() => {
        // Ignore missing registrations when the shortcut is first set up.
      });

      try {
        await registerGlobalShortcut(globalShortcut, (event) => {
          if (event.state !== "Pressed") {
            return;
          }

          handleShortcutPressed();
        });

        if (disposed) {
          await unregisterGlobalShortcut(globalShortcut).catch(() => {
            // Ignore cleanup races during hot reload and StrictMode re-mounts.
          });
        }
      } catch (error: unknown) {
        if (!disposed) {
          console.error(`Failed to register global shortcut '${globalShortcut}'.`, error);
        }
      }
    })();

    return () => {
      disposed = true;
      void unregisterGlobalShortcut(globalShortcut).catch(() => {
        // Ignore cleanup races during hot reload and StrictMode re-mounts.
      });
    };
  }, [globalShortcut, handleShortcutPressed]);
};
