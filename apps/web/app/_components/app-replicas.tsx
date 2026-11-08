/*
 * Static copies of desktop app pieces, with the classes of desktop-ui/src/components/app:
 * the project card and its environments, the domain chip, the terminal panel and the
 * AI agents settings card. The app is English only, so its UI strings stay English here.
 * Keep them in step with the app when it changes.
 */
import { RiDraggable, RiGitBranchLine, RiMoreLine } from "@remixicon/react";
import type { ReactNode } from "react";

import {
  CheckIcon,
  ChevronDownIcon,
  DotsIcon,
  GlobeIcon,
  PlayIcon,
  PlusIcon,
  SparklesIcon,
  SpinnerIcon,
  XIcon,
} from "@/components/icons";
import { cn } from "@/lib/utils";

/* --- App window (app-layout.tsx): the macOS frame around a screen --- */

export const AppFrame = ({ children, className }: { children: ReactNode; className?: string }) => (
  <div
    className={cn(
      "w-full overflow-hidden rounded-xl bg-neutral-50 p-1.5 pt-0 text-left shadow-[0_1px_0_rgba(13,13,13,0.03),0_40px_80px_-30px_rgba(13,13,13,0.45)]",
      className,
    )}
  >
    <div className="flex items-center gap-1.5 px-1.5 py-2">
      <span className="size-2.5 rounded-full bg-[#FF5C5F]" />
      <span className="size-2.5 rounded-full bg-[#FAC800]" />
      <span className="size-2.5 rounded-full bg-[#34C759]" />
    </div>
    <div className="rounded-lg bg-background p-3 md:p-4">{children}</div>
  </div>
);

/* --- Domain chip (projects-list-item-domain-chip.tsx) --- */

export type ChipState = "running" | "stopped" | "pending";

export const DomainChip = ({
  domain,
  port,
  state = "running",
  isPublic = false,
}: {
  domain: string;
  port?: string;
  state?: ChipState;
  isPublic?: boolean;
}) => {
  const reachable = state === "running";
  return (
    <span
      className={cn(
        "inline-flex h-8 max-w-full items-center gap-1 rounded-full border border-border pr-1 pl-2 text-sm text-foreground",
        reachable ? "opacity-80" : "opacity-60",
      )}
    >
      <span className={cn("size-4 shrink-0 rounded-full", reachable ? "bg-success" : "bg-neutral-400")} />
      <span className="inline-flex h-full min-w-0 items-center rounded-full px-1">
        <span className="truncate">{domain}</span>
      </span>
      {port ? (
        <span className="hidden h-5 shrink-0 items-center rounded bg-muted px-1 text-xs text-muted-foreground sm:inline-flex">
          :{port}
        </span>
      ) : null}
      {isPublic ? (
        <span className="inline-flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground">
          <GlobeIcon className="size-3.5" />
        </span>
      ) : null}
      <span className="ml-0.5 inline-flex size-6 shrink-0 items-center justify-center rounded-full text-foreground">
        {state === "pending" ? (
          <SpinnerIcon className="size-3 animate-spin" />
        ) : reachable ? (
          <XIcon className="size-3" />
        ) : (
          <PlayIcon className="size-3 translate-x-px" />
        )}
      </span>
      <span className="ml-0.5 inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground">
        <DotsIcon className="size-5 stroke-2" />
      </span>
    </span>
  );
};

/* --- Project card with its environments (projects-list-item.tsx, projects-list-item-dev-dx.tsx) --- */

export type Environment = {
  slug: string;
  branch?: string;
  services: Array<{ name?: string; domain: string; port?: string; state?: ChipState; isPublic?: boolean }>;
  open?: boolean;
};

export const ProjectCard = ({
  name,
  domains,
  environments = [],
  isPublic = false,
  selected = false,
  footer,
}: {
  name: string;
  domains: Array<{ domain: string; port?: string; state?: ChipState; isPublic?: boolean }>;
  environments?: Environment[];
  isPublic?: boolean;
  selected?: boolean;
  footer?: ReactNode;
}) => (
  <div
    className={cn(
      "relative flex gap-3 rounded-xl border border-border/30 bg-neutral-50/50 px-3 py-3 text-left",
      selected && "border-foreground/20 bg-muted/60 ring-1 ring-foreground/10",
    )}
  >
    <span className="hidden size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground sm:inline-flex">
      <RiDraggable className="size-4" />
    </span>
    <div className="flex min-w-0 flex-1 flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <span className="truncate text-lg font-medium text-foreground">{name}</span>
        {isPublic ? <GlobeIcon className="size-3.5 shrink-0 text-muted-foreground" /> : null}
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {domains.map((d) => (
          <DomainChip key={d.domain} {...d} />
        ))}
        <span className="inline-flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed border-border text-muted-foreground">
          <PlusIcon className="size-3.5" />
        </span>
      </div>
      {environments.length > 0 ? (
        <div className="mt-1.5 flex flex-col gap-0.5 border-t border-border/60 pt-1.5">
          {environments.map((env) => (
            <EnvironmentRow env={env} key={env.slug} />
          ))}
        </div>
      ) : null}
      {footer}
    </div>
    <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-md text-muted-foreground">
      <RiMoreLine className="size-4" />
    </span>
  </div>
);

const EnvironmentRow = ({ env }: { env: Environment }) => {
  const up = env.services.filter((s) => (s.state ?? "running") === "running").length;
  const total = env.services.length;
  const multi = total > 1;
  return (
    <div>
      <div className="flex w-full items-center gap-1 rounded-md py-1">
        <span className="flex min-w-0 flex-1 items-center gap-2">
          <ChevronDownIcon
            className={cn("size-3.5 shrink-0 text-muted-foreground/60", !env.open && "-rotate-90")}
          />
          <span
            className={cn(
              "size-1.5 shrink-0 rounded-full",
              up === total ? "bg-success" : up > 0 ? "bg-amber-500" : "bg-neutral-400",
            )}
          />
          <span className="shrink-0 rounded-full border border-border/60 bg-muted/60 px-1.5 py-px text-[10px] font-medium text-muted-foreground">
            worktree
          </span>
          <span className="min-w-0 truncate text-sm font-medium text-foreground">{env.slug}</span>
          {env.branch ? (
            <span className="hidden min-w-0 truncate text-xs text-muted-foreground sm:inline">{env.branch}</span>
          ) : null}
          <span className="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">
            {up}/{total}
          </span>
        </span>
        <span className="inline-flex size-6 shrink-0 items-center justify-center text-muted-foreground">
          <RiMoreLine className="size-3.5" />
        </span>
      </div>
      {env.open ? (
        <div className="ml-[7px] flex flex-col gap-2 border-l border-border/60 py-1.5 text-muted-foreground">
          {env.services.map((s) => (
            <div className="relative flex min-w-0 flex-col gap-1 pl-3" key={s.domain}>
              <span className="absolute top-4 left-0 h-px w-2.5 -translate-y-1/2 bg-border/60" />
              <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                {multi && s.name ? <span className="shrink-0 text-sm text-foreground">{s.name}</span> : null}
                <DomainChip domain={s.domain} isPublic={s.isPublic} port={s.port} state={s.state} />
              </div>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
};

/* --- Terminal panel (terminal-panel.tsx, terminal-instance.tsx) --- */

export type TerminalCheckout = { project?: string; label: string; worktree?: boolean; tabs: number; active?: boolean };

export const TerminalPanel = ({
  checkouts,
  tabs,
  lines,
}: {
  checkouts: TerminalCheckout[];
  tabs: Array<{ label: string; active?: boolean }>;
  lines: ReactNode;
}) => (
  <section className="w-full overflow-hidden rounded-lg border border-neutral-800 bg-[#111111] text-left text-neutral-100 shadow-2xl">
    <div className="flex h-full min-h-0">
      <aside className="hidden w-40 shrink-0 flex-col border-r border-white/10 bg-[#141414] sm:flex">
        <div className="flex h-10 shrink-0 items-center justify-between border-b border-white/10 bg-[#171717] pr-2 pl-3">
          <span className="text-xs font-medium text-neutral-400">Running</span>
          <PlusIcon className="size-4 text-neutral-300" />
        </div>
        <div className="space-y-2 p-1.5">
          {groupByProject(checkouts).map(([project, rows]) => (
            <div key={project ?? "none"}>
              {project ? (
                <div className="truncate px-1.5 pb-0.5 text-[11px] font-medium text-neutral-500">{project}</div>
              ) : null}
              {rows.map((row) => (
                <div
                  className={cn(
                    "flex h-7 w-full items-center gap-1.5 rounded-md px-1.5 text-xs",
                    row.active ? "bg-white/10 text-white" : "text-neutral-400",
                  )}
                  key={row.label}
                >
                  <span className="size-1.5 shrink-0 rounded-full bg-emerald-400" />
                  {row.worktree ? <RiGitBranchLine className="size-3 shrink-0 text-neutral-500" /> : null}
                  <span className="min-w-0 flex-1 truncate">{row.label}</span>
                  <span className="shrink-0 font-mono text-[11px] text-neutral-500 tabular-nums">{row.tabs}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </aside>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="flex h-10 shrink-0 items-center gap-1 border-b border-white/10 bg-[#171717] px-2">
          <div className="flex min-w-0 flex-1 items-center gap-1">
            {tabs.map((tab) => (
              <span
                className={cn(
                  "flex h-7 max-w-48 shrink-0 items-center gap-1.5 rounded-md pr-0.5 pl-2 text-xs",
                  tab.active ? "bg-white/10 text-white" : "text-neutral-400",
                )}
                key={tab.label}
              >
                <span className="size-1.5 shrink-0 rounded-full bg-emerald-400" />
                <span className="min-w-0 truncate">{tab.label}</span>
                <span
                  className={cn(
                    "inline-flex size-4 shrink-0 items-center justify-center rounded text-neutral-500",
                    !tab.active && "opacity-0",
                  )}
                >
                  <XIcon className="size-3" />
                </span>
              </span>
            ))}
            <PlusIcon className="ml-1 size-4 shrink-0 text-neutral-400" />
          </div>
          <XIcon className="size-4 text-neutral-300" />
        </div>
        <div className="min-h-[200px] flex-1 bg-[#101010] px-4 py-3 font-mono text-[12px] leading-[1.34] text-[#dddddd]">
          {lines}
        </div>
      </div>
    </div>
  </section>
);

const groupByProject = (rows: TerminalCheckout[]) => {
  const groups = new Map<string | undefined, TerminalCheckout[]>();
  for (const row of rows) groups.set(row.project, [...(groups.get(row.project) ?? []), row]);
  return [...groups.entries()];
};

/* --- AI agents settings card (settings-agents.tsx, settings-card.tsx) --- */

export type AgentsState = "ready" | "connecting" | "connected";

const AGENT_LABELS = ["Claude Code", "Codex", "Cursor"];

const Toggle = ({ title, body, on }: { title: string; body: string; on: boolean }) => (
  <div className="flex items-center gap-3">
    <div className="min-w-0 flex-1">
      <div className="text-sm font-medium text-foreground">{title}</div>
      <div className="text-xs leading-normal text-muted-foreground">{body}</div>
    </div>
    <span
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-transparent",
        on ? "bg-primary" : "bg-border/70",
      )}
    >
      <span
        className={cn(
          "inline-block size-4 rounded-full bg-background shadow-sm ring-1 ring-black/5",
          on ? "translate-x-[18px]" : "translate-x-0.5",
        )}
      />
    </span>
  </div>
);

export const AgentsSettingsCard = ({ state }: { state: AgentsState }) => {
  const connected = state === "connected";
  return (
    <div className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-5 text-left shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset]">
      <div className="flex items-center gap-5">
        <span className="flex size-10 shrink-0 items-center justify-center rounded-full bg-background text-foreground/80 ring-1 ring-border/60">
          <SparklesIcon className="size-4" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h3 className="text-[17px] font-semibold tracking-tight text-foreground">AI agents</h3>
          <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
            Connects new agents · Manual dev scripts · {connected ? "Agents connected" : AGENT_LABELS.join(", ")}
          </p>
        </div>
        <span className="inline-flex size-10 shrink-0 items-center justify-center rounded-full border border-border bg-background">
          <ChevronDownIcon className="size-4 rotate-180" />
        </span>
      </div>
      <div className="mt-5 space-y-4 border-t border-border/40 pt-5">
        <Toggle
          body="During setup and once after an update, give compatible coding agents Localify's MCP server, with every action the app has."
          on
          title="Connect agents automatically"
        />
        {connected ? null : (
          <>
            <div className="flex items-start gap-3 rounded-xl border border-border/60 bg-background/70 p-3">
              <span className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary ring-1 ring-primary/20">
                <CheckIcon className="size-4" />
              </span>
              <div className="min-w-0 flex-1 space-y-1">
                <div className="text-sm font-medium text-foreground">Ready to connect</div>
                <div className="text-xs leading-normal text-muted-foreground">
                  Claude Code, Codex, Cursor will get every Localify action: start and stop services, manage
                  domains, and clean up worktrees.
                </div>
                <div className="flex flex-wrap gap-1.5 pt-1">
                  {AGENT_LABELS.map((label) => (
                    <span className="rounded-full bg-muted px-2 py-0.5 text-[11px] font-medium text-muted-foreground" key={label}>
                      {label}
                    </span>
                  ))}
                </div>
              </div>
            </div>
            <div className="flex justify-end">
              <span className="inline-flex h-9 items-center gap-2 rounded-full bg-secondary px-4 text-sm font-medium text-secondary-foreground">
                {state === "connecting" ? <SpinnerIcon className="size-4 animate-spin" /> : null}
                {state === "connecting" ? "Connecting..." : "Connect agents"}
              </span>
            </div>
          </>
        )}
      </div>
    </div>
  );
};

/* Toast shown after connecting (sonner style in the app). */
export const ConnectedToast = () => (
  <div className="flex w-full max-w-xs items-start gap-2.5 rounded-xl border border-border bg-background px-3.5 py-3 text-left shadow-lg">
    <CheckIcon className="mt-0.5 size-4 shrink-0 text-emerald-600" />
    <div className="min-w-0">
      <p className="text-[13px] font-medium text-foreground">Agents connected to Localify</p>
      <p className="text-xs text-muted-foreground">Claude Code, Codex, Cursor. Restart open agent sessions to load it.</p>
    </div>
  </div>
);
