import { Link, useRouterState } from "@tanstack/react-router";
import { useBootstrap, useStartDaemon, useStopDaemon } from "../lib/queries";
import { Button } from "./ui/button";
import { cn } from "../lib/cn";
import { CogFourIcon, FileTextIcon, HexagonIcon } from "./icons";

const tabs = [
  { to: "/projects", label: "Projects", icon: HexagonIcon },
  { to: "/logs", label: "Logs", icon: FileTextIcon },
  { to: "/settings", label: "Settings", icon: CogFourIcon },
] as const;

export function TopBar() {
  const { data } = useBootstrap();
  const startDaemon = useStartDaemon();
  const stopDaemon = useStopDaemon();
  const {
    location: { pathname },
  } = useRouterState();

  const running = data?.daemon.running ?? false;
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const appName = data?.environment.appName ?? "Localify";
  const daemonError = startDaemon.error ?? stopDaemon.error;
  const daemonErrorMessage = daemonError instanceof Error ? daemonError.message : null;

  return (
    <div className="fixed dark bottom-12 left-1/2 -translate-x-1/2 w-full max-w-4xl rounded-full bg-background text-foreground">
      <div className="mx-auto flex h-14 w-full max-w-4xl items-center gap-6 px-8">
        <div className="flex items-center gap-1">
          {tabs.map((tab) => {
            const active = pathname.startsWith(tab.to);
            return (
              <Link
                key={tab.to}
                to={tab.to}
                className={cn(
                  "rounded-full p-2 text-sm font-medium transition-colors ",
                  active
                    ? "bg-[--color-surface] text-[--color-ink]"
                    : "text-[--color-muted] hover:text-[--color-ink]",
                )}
              >
                <tab.icon className="size-4 stroke-2" />
              </Link>
            );
          })}
        </div>

        <div className="ml-auto flex items-center gap-3">
          <div className="flex items-center gap-2 text-xs text-[--color-muted]">
            <span
              className={cn(
                "h-2 w-2 rounded-full",
                running ? "bg-[--color-accent]" : "bg-[--color-muted]",
              )}
            />
            <span>{running ? "Running" : "Stopped"}</span>
          </div>
          {running ? (
            <Button
              size="sm"
              variant="secondary"
              loading={stopDaemon.isPending}
              onClick={() => stopDaemon.mutate()}
            >
              Stop
            </Button>
          ) : (
            <Button
              size="sm"
              variant="primary"
              loading={startDaemon.isPending}
              onClick={() => startDaemon.mutate({ verbose: false })}
            >
              Start
            </Button>
          )}
        </div>
      </div>
      {daemonErrorMessage && (
        <div className="border-t border-[--color-border] bg-[--color-surface]">
          <div className="mx-auto w-full max-w-4xl px-8 py-2">
            <p className="text-xs text-[--color-danger]">{daemonErrorMessage}</p>
          </div>
        </div>
      )}
    </div>
  );
}
