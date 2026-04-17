import { useHotkey } from "@tanstack/react-hotkeys";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { toRegisterableHotkey } from "@/lib/hotkeys";
import { useAppHotkeys } from "@/providers/hotkeys-provider";

import {
  queueProjectCreateDialogOpen,
  requestProjectCreateDialogOpen,
} from "./util.project-create-dialog";

export const useProjectCreateHotkey = () => {
  const navigate = useNavigate();
  const { hotkeys } = useAppHotkeys();
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  });

  useHotkey(
    toRegisterableHotkey(hotkeys.createProject),
    () => {
      if (pathname.startsWith("/projects")) {
        requestProjectCreateDialogOpen();
        return;
      }

      queueProjectCreateDialogOpen();
      void navigate({ to: "/projects" });
    },
    {
      preventDefault: true,
    },
  );
};
