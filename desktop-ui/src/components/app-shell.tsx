import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";

import { bootstrapQueries } from "@/lib/queries";
import { AppUpdateProvider } from "@/providers/app-update-provider";

import { Onboarding } from "./onboarding";
import { AppStartupScreen } from "./shared/app-startup-screen";

interface AppShellProps {
  children: ReactNode;
}

const ONBOARDING_COMPLETED_STORAGE_KEY = "localify.onboarding-completed";
const DEFAULT_DESKTOP_NAME = "Localify Desktop";

function loadStoredOnboardingCompleted(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  return window.localStorage.getItem(ONBOARDING_COMPLETED_STORAGE_KEY) === "1";
}

export function AppShell({ children }: AppShellProps) {
  const { data, isLoading, error } = useQuery(bootstrapQueries.read());
  const [onboardingCompleted, setOnboardingCompleted] = useState(loadStoredOnboardingCompleted);

  useEffect(() => {
    if (data?.daemon.caInstalled) {
      setOnboardingCompleted(true);
      window.localStorage.setItem(ONBOARDING_COMPLETED_STORAGE_KEY, "1");
    }
  }, [data?.daemon.caInstalled]);

  if (isLoading && !data) {
    return onboardingCompleted ? (
      <AppUpdateProvider desktopName={DEFAULT_DESKTOP_NAME} enabled={false}>
        <>{children}</>
      </AppUpdateProvider>
    ) : null;
  }

  if (error && !data) {
    return (
      <AppStartupScreen
        eyebrow="Desktop backend"
        title="Cannot reach the desktop backend"
        message="Localify could not finish booting the desktop shell."
        error={error.message}
      />
    );
  }

  if (!data) return null;

  const handleOnboardingActivated = () => {
    setOnboardingCompleted(true);
    window.localStorage.setItem(ONBOARDING_COMPLETED_STORAGE_KEY, "1");
  };

  return (
    <AppUpdateProvider
      desktopName={data.environment.desktopName}
      enabled={data.daemon.caInstalled || onboardingCompleted}
    >
      {!data.daemon.caInstalled && !onboardingCompleted ? (
        <Onboarding bootstrap={data} onActivated={handleOnboardingActivated} />
      ) : (
        <>{children}</>
      )}
    </AppUpdateProvider>
  );
}
