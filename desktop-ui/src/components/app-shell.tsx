import type { ReactNode } from "react";
import { useBootstrap } from "../lib/queries";
import { Onboarding } from "./onboarding";

interface AppShellProps {
  children: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const { data, isLoading, error } = useBootstrap();

  if (isLoading && !data) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-[--color-muted]">
        Loading Roxy…
      </div>
    );
  }

  if (error && !data) {
    return (
      <div className="flex h-full items-center justify-center px-8 text-center">
        <div className="max-w-md space-y-2">
          <h2 className="text-base font-semibold text-[--color-danger]">
            Cannot reach the Roxy backend
          </h2>
          <p className="text-sm text-[--color-muted]">{error.message}</p>
        </div>
      </div>
    );
  }

  if (!data) return null;

  if (!data.daemon.caInstalled) {
    return <Onboarding bootstrap={data} />;
  }

  return <>{children}</>;
}
