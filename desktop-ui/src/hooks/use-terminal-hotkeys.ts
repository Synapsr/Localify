import { useHotkey } from "@tanstack/react-hotkeys";
import { toast } from "sonner";

import { toRegisterableHotkey } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";
import { useTerminal } from "@/providers/terminal-provider";

export const useTerminalHotkeys = () => {
  const { hotkeys } = useAppHotkeys();
  const {
    closeSession,
    isOpen,
    newTabFromActiveTarget,
    openSelectedTarget,
    selectNextSession,
    selectPreviousSession,
    togglePanel,
  } = useTerminal();

  useHotkey(
    toRegisterableHotkey(hotkeys.toggleTerminal),
    () => {
      if (isOpen) {
        togglePanel();
        return;
      }

      void openSelectedTarget().catch((error) => {
        toast.error("Open terminal failed", {
          description: error instanceof Error ? error.message : String(error),
        });
      });
    },
    { preventDefault: true },
  );

  useHotkey(
    toRegisterableHotkey(hotkeys.newTerminalTab),
    () => {
      if (!isOpen) {
        togglePanel();
        return;
      }

      void newTabFromActiveTarget();
    },
    { preventDefault: true },
  );

  useHotkey(
    toRegisterableHotkey(hotkeys.closeTerminalTab),
    () => {
      if (!isOpen) return;
      void closeSession();
    },
    { preventDefault: true },
  );

  useHotkey(
    toRegisterableHotkey(hotkeys.previousTerminalTab),
    () => {
      if (!isOpen) return;
      selectPreviousSession();
    },
    { preventDefault: true },
  );

  useHotkey(
    toRegisterableHotkey(hotkeys.nextTerminalTab),
    () => {
      if (!isOpen) return;
      selectNextSession();
    },
    { preventDefault: true },
  );
};
