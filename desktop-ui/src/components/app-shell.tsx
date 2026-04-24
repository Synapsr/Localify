import type { ReactNode } from "react";

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { bootstrapQueries } from "@/lib/queries";
import {
  loadStoredOnboardingCompleted,
  markStoredOnboardingCompleted,
} from "@/lib/util.onboarding-state";
import { AppUpdateProvider } from "@/providers/app-update-provider";

import { SpinnerIcon } from "./icons";
import { Onboarding } from "./onboarding";
import { AppStartupScreen } from "./shared/app-startup-screen";

interface AppShellProps {
  children: ReactNode;
}

const DEFAULT_DESKTOP_NAME = "Localify";

export function AppShell({ children }: AppShellProps) {
  const { data, isLoading, error } = useQuery(bootstrapQueries.read());
  const [onboardingCompleted, setOnboardingCompleted] = useState(loadStoredOnboardingCompleted);

  useEffect(() => {
    if (data?.daemon.caInstalled) {
      setOnboardingCompleted(true);
      markStoredOnboardingCompleted();
    }
  }, [data?.daemon.caInstalled]);

  if (isLoading && !data) {
    return onboardingCompleted ? (
      <AppUpdateProvider desktopName={DEFAULT_DESKTOP_NAME} enabled={false}>
        <>{children}</>
      </AppUpdateProvider>
    ) : (
      <div className="h-full w-full bg-background">
        <SpinnerIcon className="animate-spin" />
      </div>
    );
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

  if (!data) {
    return (
      <AppStartupScreen
        eyebrow="Desktop startup"
        title="Waiting for Localify"
        message="The desktop shell is still waiting for startup data."
      />
    );
  }

  const handleOnboardingActivated = () => {
    setOnboardingCompleted(true);
    markStoredOnboardingCompleted();
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
