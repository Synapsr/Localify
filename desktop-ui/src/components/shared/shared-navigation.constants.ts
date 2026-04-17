import type { AppHotkeyId } from "@/lib/hotkeys";

import {
  CogFourSolidIcon,
  FileTextSolidIcon,
  HexagonSolidIcon,
} from "@/components/icons";

export const sharedNavigationTabs = [
  {
    to: "/projects",
    label: "Projects",
    icon: HexagonSolidIcon,
    hotkeyId: "navigateProjects" satisfies AppHotkeyId,
  },
  {
    to: "/logs",
    label: "Logs",
    icon: FileTextSolidIcon,
    hotkeyId: "navigateLogs" satisfies AppHotkeyId,
  },
  {
    to: "/settings",
    label: "Settings",
    icon: CogFourSolidIcon,
    hotkeyId: "navigateSettings" satisfies AppHotkeyId,
  },
] as const;
