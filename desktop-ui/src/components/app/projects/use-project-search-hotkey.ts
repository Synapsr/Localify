import { useHotkey } from "@tanstack/react-hotkeys";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { toRegisterableHotkey } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import {
  queueProjectsSearchFocus,
  requestProjectsSearchFocus,
} from "./util.project-search-focus";

export const useProjectSearchHotkey = () => {
  const navigate = useNavigate();
  const { hotkeys } = useAppHotkeys();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });

  useHotkey(
    toRegisterableHotkey(hotkeys.searchProjects),
    () => {
      if (pathname.startsWith("/projects")) {
        requestProjectsSearchFocus();
        return;
      }

      queueProjectsSearchFocus();
      void navigate({ to: "/projects" });
    },
    {
      preventDefault: true,
    },
  );
};
