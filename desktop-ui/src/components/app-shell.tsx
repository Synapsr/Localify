import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { bootstrapQueries } from "../lib/queries";
import { Onboarding } from "./onboarding";

interface AppShellProps {
  children: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const { data, isLoading, error } = useQuery(bootstrapQueries.read());

  if (isLoading && !data) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-[--color-muted]">
        Loading…
      </div>
    );
  }

  if (error && !data) {
    return (
      <div className="flex h-full items-center justify-center px-8 text-center">
        <div className="max-w-md space-y-2">
          <h2 className="text-base font-semibold text-[--color-danger]">
            Cannot reach the desktop backend
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
