import type { ReactNode } from "react";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import { bootstrapQueries, systemMutations } from "@/lib/queries";
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
  const queryClient = useQueryClient();
  const { data, isPending, error } = useQuery(bootstrapQueries.read());
  const restartHelper = useMutation(systemMutations.restartHelper(queryClient));
  const [onboardingCompleted, setOnboardingCompleted] = useState(loadStoredOnboardingCompleted);

  useEffect(() => {
    if (data?.daemon.caInstalled) {
      setOnboardingCompleted(true);
      markStoredOnboardingCompleted();
    }
  }, [data?.daemon.caInstalled]);

  const handleRestartHelper = () => {
    restartHelper.mutate(undefined, {
      onSuccess: () => {
        toast.success("Background helper restarted");
      },
      onError: (restartError) => {
        toast.error("Could not restart the background helper", {
          description: restartError.message,
        });
      },
    });
  };

  if (error && !data) {
    return (
      <AppStartupScreen
        eyebrow="Desktop backend"
        title="Cannot reach the desktop backend"
        message="Localify could not finish booting the desktop shell."
        error={error.message}
        primaryActionLabel={
          restartHelper.isPending ? "Repairing background helper..." : "Repair background helper"
        }
        primaryActionLoading={restartHelper.isPending}
        onPrimaryAction={handleRestartHelper}
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
};
