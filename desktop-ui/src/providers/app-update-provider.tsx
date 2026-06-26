import { confirm, message } from "@tauri-apps/plugin-dialog";
import {
  createContext,
  type PropsWithChildren,
  useEffect,
  useEffectEvent,
  useMemo,
  useRef,
  useState,
} from "react";

import { tauriClient } from "@/lib/tauri-client";
import type { AppUpdateInfoDto } from "@/types/roxy";

export type AppUpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "installing"
  | "upToDate"
  | "unconfigured"
  | "error";

interface AppUpdateState {
  status: AppUpdateStatus;
  availableUpdate: AppUpdateInfoDto | null;
  errorMessage: string | null;
}

interface AppUpdateContextValue extends AppUpdateState {
  isChecking: boolean;
  isInstalling: boolean;
  checkForUpdates: () => Promise<void>;
  installUpdate: () => Promise<void>;
}

interface AppUpdateProviderProps extends PropsWithChildren {
  desktopName: string;
  enabled: boolean;
}

const defaultState: AppUpdateState = {
  status: "idle",
  availableUpdate: null,
  errorMessage: null,
};

const UPDATE_CHECK_TIMEOUT_MS = 30_000;
const UPDATE_INSTALL_TIMEOUT_MS = 5 * 60_000;

export const AppUpdateContext = createContext<AppUpdateContextValue | null>(null);

async function withTimeout<T>(
  promise: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(message)), timeoutMs);
  });

  try {
    return await Promise.race([promise, timeout]);
  } finally {
    if (timeoutId) {
      clearTimeout(timeoutId);
    }
  }
}

function resolveErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) {
    return error.message;
  }

  return "Localify could not download the latest desktop update.";
}

async function confirmUpdateInstall(
  desktopName: string,
  update: AppUpdateInfoDto,
): Promise<boolean> {
  return confirm(
    `Version ${update.version} is available.\n\nDownload and install it now? ${desktopName} will restart automatically when the install finishes.`,
    {
      title: "Update Available",
      kind: "info",
      okLabel: "Download",
      cancelLabel: "Not now",
    },
  );
}

export const AppUpdateProvider = ({
  children,
  desktopName,
  enabled,
}: AppUpdateProviderProps) => {
  const [state, setState] = useState<AppUpdateState>(defaultState);
  const backgroundCheckStartedRef = useRef(false);
  const checkInFlightRef = useRef(false);
  const installInFlightRef = useRef(false);

  const installUpdateEvent = useEffectEvent(
    async (update: AppUpdateInfoDto | null, source: "background" | "manual") => {
      if (!update || installInFlightRef.current) {
        return;
      }

      installInFlightRef.current = true;
      setState({
        status: "installing",
        availableUpdate: update,
        errorMessage: null,
      });

      try {
        await withTimeout(
          tauriClient.installAppUpdate(),
          UPDATE_INSTALL_TIMEOUT_MS,
          "Update download timed out. Check your connection and try again.",
        );
        setState({
          status: "upToDate",
          availableUpdate: null,
          errorMessage: null,
        });
      } catch (error) {
        const errorMessage = resolveErrorMessage(error);

        setState({
          status: "error",
          availableUpdate: update,
          errorMessage,
        });

        await message(errorMessage, {
          title: source === "manual" ? "Update Failed" : "Update Install Failed",
          kind: "error",
        });
      } finally {
        installInFlightRef.current = false;
      }
    },
  );

  const checkForUpdatesEvent = useEffectEvent(async (source: "background" | "manual") => {
    if (!enabled || checkInFlightRef.current || installInFlightRef.current) {
      return;
    }

    checkInFlightRef.current = true;
    setState((current) => ({
      ...current,
      status: "checking",
      errorMessage: null,
    }));

    try {
      const updateCheck = await withTimeout(
        tauriClient.checkAppUpdate(),
        UPDATE_CHECK_TIMEOUT_MS,
        "Update check timed out. Try again in a moment.",
      );

      if (!updateCheck.configured) {
        setState({
          status: "unconfigured",
          availableUpdate: null,
          errorMessage: null,
        });

        if (source === "manual") {
          await message("Desktop updates are not configured for this build.", {
            title: "Updates Unavailable",
            kind: "info",
          });
        }

        return;
      }

      if (!updateCheck.update) {
        setState({
          status: "upToDate",
          availableUpdate: null,
          errorMessage: null,
        });

        if (source === "manual") {
          await message(`${desktopName} is already up to date.`, {
            title: "You're Up to Date",
            kind: "info",
          });
        }

        return;
      }

      setState({
        status: "available",
        availableUpdate: updateCheck.update,
        errorMessage: null,
      });

      const confirmed = await confirmUpdateInstall(desktopName, updateCheck.update);
      if (!confirmed) {
        return;
      }

      await installUpdateEvent(updateCheck.update, source);
    } catch (error) {
      const errorMessage = resolveErrorMessage(error);

      setState((current) => ({
        status: "error",
        availableUpdate: current.availableUpdate,
        errorMessage,
      }));

      if (source === "manual") {
        await message(errorMessage, {
          title: "Update Check Failed",
          kind: "error",
        });
      }
    } finally {
      checkInFlightRef.current = false;
    }
  });

  useEffect(() => {
    if (!enabled || backgroundCheckStartedRef.current) {
      return;
    }

    backgroundCheckStartedRef.current = true;
    void checkForUpdatesEvent("background");
  }, [enabled]);

  const value = useMemo<AppUpdateContextValue>(
    () => ({
      ...state,
      isChecking: state.status === "checking",
      isInstalling: state.status === "installing",
      checkForUpdates: async () => {
        await checkForUpdatesEvent("manual");
      },
      installUpdate: async () => {
        await installUpdateEvent(state.availableUpdate, "manual");
      },
    }),
    [state],
  );

  return <AppUpdateContext.Provider value={value}>{children}</AppUpdateContext.Provider>;
};
