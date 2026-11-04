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

export const AppShell = ({ children }: AppShellProps) => {
  const { data, isPending, error, refetch, isFetching } = useQuery(bootstrapQueries.read());
  const [onboardingCompleted, setOnboardingCompleted] = useState(loadStoredOnboardingCompleted);

  useEffect(() => {
    if (data?.daemon.caInstalled) {
      setOnboardingCompleted(true);
      markStoredOnboardingCompleted();
    }
  }, [data?.daemon.caInstalled]);

  if (error && !data) {
    return (
      <AppStartupScreen
        eyebrow="Desktop backend"
        title="Cannot reach the desktop backend"
        message="Localify could not finish booting the desktop shell."
        error={error.message}
        primaryActionLabel={isFetching ? "Trying again..." : "Try again"}
        primaryActionLoading={isFetching}
        onPrimaryAction={() => void refetch()}
      />
    );
  }

  if (isPending && !data) {
    return onboardingCompleted ? (
      <AppStartupScreen
        eyebrow="Desktop startup"
        title="Starting Localify"
        message="Localify is reading your desktop configuration."
      />
    ) : (
      <div className="flex h-full w-full items-center justify-center bg-background">
        <SpinnerIcon className="animate-spin" />
      </div>
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

  // After an update the first run is long past, but the old root helper may still
  // need replacing, so that takes the same one approval.
  const needsSetup =
    data.environment.systemSetupRequired || (!data.daemon.caInstalled && !onboardingCompleted);

  const handleOnboardingActivated = () => {
    setOnboardingCompleted(true);
    markStoredOnboardingCompleted();
  };

  return (
    <AppUpdateProvider
      desktopName={data.environment.desktopName}
      enabled={data.daemon.caInstalled || onboardingCompleted}
    >
      {needsSetup ? (
        <Onboarding
          bootstrap={data}
          onActivated={handleOnboardingActivated}
          startAtActivation={onboardingCompleted}
        />
      ) : (
        <>{children}</>
      )}
    </AppUpdateProvider>
  );
};
