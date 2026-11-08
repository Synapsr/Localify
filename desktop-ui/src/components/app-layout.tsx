import { useAppNavigationHotkeys } from "@/hooks/use-app-navigation-hotkeys";
import { useTerminalHotkeys } from "@/hooks/use-terminal-hotkeys";
import { useThemeShortcut } from "@/hooks/use-theme-shortcut";
import { ProjectOrderProvider } from "@/providers/project-order-provider";
import { RemoteProvider } from "@/providers/remote-provider";
import { TerminalProvider, useTerminal } from "@/providers/terminal-provider";

import { HttpsTrustNotice } from "./app/https-trust-notice";
import { useProjectCreateHotkey } from "./app/projects/use-project-create-hotkey";
import { useProjectSearchHotkey } from "./app/projects/use-project-search-hotkey";
import { TerminalPanel } from "./app/terminal/terminal-panel";
import { DesktopAnalytics } from "./shared/desktop-analytics";
import { PageTransition } from "./shared/page-transition";
import { SharedNavigation } from "./shared/shared-navigation";

export function AppLayout() {
  return (
    <ProjectOrderProvider>
      <TerminalProvider>
        <RemoteProvider>
          <AppLayoutContent />
        </RemoteProvider>
      </TerminalProvider>
    </ProjectOrderProvider>
  );
}

function AppLayoutContent() {
  useAppNavigationHotkeys();
  useProjectCreateHotkey();
  useProjectSearchHotkey();
  useTerminalHotkeys();
  useThemeShortcut();
  const { isOpen } = useTerminal();

  return (
    <div
      data-tauri-drag-region
      className="flex h-full flex-col bg-neutral-50 p-1.5 pt-8 dark:bg-neutral-950/40 "
    >
      <div className="flex h-full flex-col rounded-lg bg-background ">
        <DesktopAnalytics />
        {!isOpen ? <SharedNavigation /> : null}
        <HttpsTrustNotice />
        <main className="flex-1 overflow-hidden  rounded-lg">
          <PageTransition />
        </main>
        <TerminalPanel />
      </div>
    </div>
  );
}
