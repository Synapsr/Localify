import { useAppNavigationHotkeys } from "@/hooks/use-app-navigation-hotkeys";
import { useThemeShortcut } from "@/hooks/use-theme-shortcut";

import { useProjectCreateHotkey } from "./app/projects/use-project-create-hotkey";
import { useProjectSearchHotkey } from "./app/projects/use-project-search-hotkey";
import { DesktopAnalytics } from "./shared/desktop-analytics";
import { PageTransition } from "./shared/page-transition";
import { SharedNavigation } from "./shared/shared-navigation";

export function AppLayout() {
  useAppNavigationHotkeys();
  useProjectCreateHotkey();
  useProjectSearchHotkey();
  useThemeShortcut();

  return (
    <div
      data-tauri-drag-region
      className="flex h-full flex-col bg-neutral-50 p-1.5 pt-8 dark:bg-neutral-950/40 "
    >
      <div className="flex h-full flex-col rounded-lg bg-background ">
        <DesktopAnalytics />
        <SharedNavigation />
        <main className="flex-1 overflow-hidden  rounded-lg">
          <PageTransition />
        </main>
      </div>
    </div>
  );
}
