import { useEffect, useMemo, useState } from "react";

import type { AppUpdateCheckDto, AppUpdateInfoDto } from "@/types/roxy";
import { tauriClient } from "@/lib/tauri-client";

type AppUpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "installing"; update: AppUpdateInfoDto }
  | { kind: "error"; update: AppUpdateInfoDto | null; message: string };

interface UseAppUpdateResult {
  state: AppUpdateState;
  retry: () => void;
  skip: () => void;
}

function resolveErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }

  return "Localify could not download the latest desktop update.";
}

export function useAppUpdate(enabled: boolean): UseAppUpdateResult {
  const [state, setState] = useState<AppUpdateState>({ kind: "idle" });
  const [attempt, setAttempt] = useState(0);
  const [skipped, setSkipped] = useState(false);

  useEffect(() => {
    if (!enabled || skipped) {
      setState({ kind: "idle" });
      return;
    }

    let cancelled = false;

    const run = async () => {
      setState({ kind: "checking" });

      let updateCheck: AppUpdateCheckDto;
      try {
        updateCheck = await tauriClient.checkAppUpdate();
      } catch (error) {
        if (!cancelled) {
          setState({
            kind: "error",
            update: null,
            message: resolveErrorMessage(error),
          });
        }
        return;
      }

      if (cancelled || !updateCheck.configured || !updateCheck.update) {
        if (!cancelled) {
          setState({ kind: "idle" });
        }
        return;
      }

      setState({ kind: "installing", update: updateCheck.update });

      try {
        await tauriClient.installAppUpdate();
      } catch (error) {
        if (!cancelled) {
          setState({
            kind: "error",
            update: updateCheck.update,
            message: resolveErrorMessage(error),
          });
        }
      }
    };

    void run();

    return () => {
      cancelled = true;
    };
  }, [attempt, enabled, skipped]);

  return useMemo(
    () => ({
      state,
      retry: () => {
        setSkipped(false);
        setAttempt((value) => value + 1);
      },
      skip: () => {
        setSkipped(true);
        setState({ kind: "idle" });
      },
    }),
    [state],
  );
}
