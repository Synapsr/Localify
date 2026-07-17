import { createContext, PropsWithChildren, useContext, useEffect, useMemo, useState } from "react";

import type { AppHotkeys, AppHotkeyId } from "@/lib/hotkeys";

import { defaultAppHotkeys } from "@/lib/hotkeys";
import { parseStoredHotkeys } from "@/lib/validators/validator.hotkeys";

const HOTKEYS_STORAGE_KEY = "localify.app-hotkeys";

interface HotkeysContextValue {
  hotkeys: AppHotkeys;
  resetHotkeys: () => void;
  setHotkeys: (nextHotkeys: AppHotkeys) => void;
  updateHotkey: (id: AppHotkeyId, hotkey: string) => void;
}

const HotkeysContext = createContext<HotkeysContextValue | null>(null);

function loadStoredHotkeys(): AppHotkeys {
  if (typeof window === "undefined") {
    return defaultAppHotkeys;
  }

  try {
    const raw = window.localStorage.getItem(HOTKEYS_STORAGE_KEY);
    if (!raw) {
      return defaultAppHotkeys;
    }

    return parseStoredHotkeys(JSON.parse(raw));
  } catch {
    return defaultAppHotkeys;
  }
}

export const HotkeysProvider = ({ children }: PropsWithChildren) => {
  const [hotkeys, setHotkeysState] = useState<AppHotkeys>(loadStoredHotkeys);

  useEffect(() => {
    try {
      window.localStorage.setItem(HOTKEYS_STORAGE_KEY, JSON.stringify(hotkeys));
    } catch {
      // localStorage unavailable — ignore
    }
  }, [hotkeys]);

  const value = useMemo<HotkeysContextValue>(
    () => ({
      hotkeys,
      resetHotkeys: () => {
        setHotkeysState(defaultAppHotkeys);
      },
      setHotkeys: (nextHotkeys) => {
        setHotkeysState(nextHotkeys);
      },
      updateHotkey: (id, hotkey) => {
        setHotkeysState((current) => ({
          ...current,
          [id]: hotkey,
        }));
      },
    }),
    [hotkeys],
  );

  return <HotkeysContext.Provider value={value}>{children}</HotkeysContext.Provider>;
};

export function useAppHotkeys() {
  const context = useContext(HotkeysContext);

  if (!context) {
    throw new Error("useAppHotkeys must be used within HotkeysProvider");
  }

  return context;
}
