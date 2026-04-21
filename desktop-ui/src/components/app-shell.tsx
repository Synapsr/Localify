import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";

import { useAppUpdate } from "@/hooks/use-app-update";
import { bootstrapQueries } from "@/lib/queries";

import { Onboarding } from "./onboarding";
import { AppStartupScreen } from "./shared/app-startup-screen";

interface AppShellProps {
  children: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const { data, isLoading, error } = useQuery(bootstrapQueries.read());
  const appUpdate = useAppUpdate(Boolean(data));
  const [onboardingCompleted, setOnboardingCompleted] = useState(false);

  useEffect(() => {
    if (data?.daemon.caInstalled) {
      setOnboardingCompleted(true);
    }
  }, [data?.daemon.caInstalled]);

  if (isLoading && !data) {
    return (
      <AppStartupScreen
        title="Loading Localify"
        message="Connecting to the desktop backend and reading your local routing state."
      />
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

  if (!data) return null;

  if (appUpdate.state.kind === "checking") {
    return (
      <AppStartupScreen
        eyebrow="Desktop update"
        title="Checking for updates"
        message="Looking for a newer Localify Desktop build before opening the app."
      />
    );
  }

  if (appUpdate.state.kind === "installing") {
    return (
      <AppStartupScreen
        eyebrow="Desktop update"
        title={`Installing ${data.environment.desktopName} ${appUpdate.state.update.version}`}
        message="Downloading and applying the latest desktop update."
        detail="The app will relaunch automatically when the install is finished."
      />
    );
  }

  if (appUpdate.state.kind === "error") {
    return (
      <AppStartupScreen
        eyebrow="Desktop update"
        title="Update failed"
        message="Localify could not install the latest desktop build."
        detail={
          appUpdate.state.update
            ? `Update ${appUpdate.state.update.version} is available, but this install attempt failed.`
            : "No update was applied during startup."
        }
        error={appUpdate.state.message}
        primaryActionLabel="Retry update"
        secondaryActionLabel="Continue without updating"
        onPrimaryAction={appUpdate.retry}
        onSecondaryAction={appUpdate.skip}
      />
    );
  }

  if (!data.daemon.caInstalled && !onboardingCompleted) {
    return <Onboarding bootstrap={data} onActivated={() => setOnboardingCompleted(true)} />;
  }

  return <>{children}</>;
}
