import { useHotkeys } from "@tanstack/react-hotkeys";
import { useNavigate, useRouterState } from "@tanstack/react-router";

import { sharedNavigationTabs } from "@/components/shared/shared-navigation.constants";
import { toRegisterableHotkey } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

export const useAppNavigationHotkeys = () => {
  const navigate = useNavigate();
  const { hotkeys } = useAppHotkeys();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });

  useHotkeys(
    sharedNavigationTabs.map((tab) => ({
      hotkey: toRegisterableHotkey(hotkeys[tab.hotkeyId]),
      callback: () => {
        if (pathname.startsWith(tab.to)) {
          return;
        }

        void navigate({ to: tab.to });
      },
    })),
    {
      preventDefault: true,
    },
  );
};
