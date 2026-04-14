import { Link, useRouterState } from "@tanstack/react-router";
import { useBootstrap, useStartDaemon, useStopDaemon } from "../lib/queries";
import { Button } from "./ui/button";
import { cn } from "../lib/cn";

const tabs = [
  { to: "/domains", label: "Domains" },
  { to: "/logs", label: "Logs" },
  { to: "/settings", label: "Settings" },
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

  return (
    <header className="border-b border-[--color-border] bg-[--color-bg]">
      <div className="mx-auto flex h-14 w-full max-w-4xl items-center gap-6 px-8">
        <div className="flex items-center gap-2.5">
          <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-[--color-ink] text-[--color-bg] font-mono text-xs font-bold">
            R
          </div>
          <div className="flex items-baseline gap-1.5">
            <span className="text-sm font-semibold text-[--color-ink]">
              Roxy
            </span>
            {suffix && (
              <span className="font-mono text-xs text-[--color-muted]">
                {suffix}
              </span>
            )}
          </div>
        </div>

        <nav className="flex items-center gap-1">
          {tabs.map((tab) => {
            const active = pathname.startsWith(tab.to);
            return (
              <Link
                key={tab.to}
                to={tab.to}
                className={cn(
                  "rounded-md px-3 py-1.5 text-sm font-medium transition-colors",
                  active
                    ? "bg-[--color-surface] text-[--color-ink]"
                    : "text-[--color-muted] hover:text-[--color-ink]",
                )}
              >
                {tab.label}
              </Link>
            );
          })}
        </nav>

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
    </header>
  );
}
