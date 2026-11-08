"use client";

import { RiGitBranchLine } from "@remixicon/react";
import { formatForDisplay } from "@tanstack/react-hotkeys";
import { useQuery } from "@tanstack/react-query";
import {
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
} from "react";
import { toast } from "sonner";

import type {
  BootstrapDto,
  ProjectEnvironmentInfoDto,
  ProjectInfoDto,
  ProjectServiceInfoDto,
  TerminalTargetRequest,
} from "@/types/localify";

import { LaunchProfileBadge } from "@/components/app/launch-profiles/launch-profile-badge";
import { LaunchProfileMenu } from "@/components/app/launch-profiles/launch-profile-menu";
import {
  serviceLaunchProfiles,
  type ServiceLaunchProfiles,
} from "@/components/app/launch-profiles/util.launch-profiles";
import {
  ChevronDownIcon,
  PlayIcon,
  PlusIcon,
  SearchIcon,
  TerminalIcon,
  XIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { bootstrapQueries } from "@/lib/queries";
import {
  groupTerminalSessions,
  type TerminalSessionState,
  type TerminalTabEnvironment,
  type TerminalTabItem,
  type TerminalTabProject,
} from "@/lib/util.terminal-sessions";
import { cn } from "@/lib/utils";
import { useAppHotkeys } from "@/providers/hotkeys-provider";
import { useSharedProjectOrder } from "@/providers/project-order-provider";
import { useTerminal } from "@/providers/terminal-provider";

import { TerminalInstance } from "./terminal-instance";
import {
  ShellEnvironmentWarning,
  TerminalPanelMenu,
  TerminalRunEndBar,
  TerminalSharedInputNote,
} from "./terminal-run-status";

export const TerminalPanel = () => {
  const {
    activeSession,
    closePanel,
    isOpen,
    outputBySessionId,
    panelHeight,
    sessions,
    setPanelHeight,
  } = useTerminal();

  if (!isOpen) {
    return null;
  }

  const currentSession = activeSession ?? sessions[0] ?? null;
  const projects = groupTerminalSessions(sessions);
  const currentProject = projects.find(
    (project) => project.projectId === currentSession?.target.projectId,
  );
  const currentEnvironment = currentProject?.environments.find(
    (environment) => environment.slug === currentSession?.target.environmentSlug,
  );
  const environmentCount = projects.reduce(
    (count, project) => count + project.environments.length,
    0,
  );
  const showSidebar = environmentCount > 1;

  const startResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startY = event.clientY;
    const startHeight = panelHeight;

    const onPointerMove = (moveEvent: PointerEvent) => {
      setPanelHeight(startHeight + (startY - moveEvent.clientY));
    };

    const onPointerUp = () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    };

    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
  };

  return (
    <section
      className="fixed inset-x-1.5 bottom-1.5 z-50 overflow-hidden rounded-lg border border-neutral-800 bg-[#111111] text-neutral-100 shadow-2xl"
      style={{ height: panelHeight }}
      aria-label="Terminal"
    >
      <div
        role="separator"
        aria-orientation="horizontal"
        onPointerDown={startResize}
        className="absolute inset-x-0 top-0 z-20 h-1 cursor-ns-resize bg-transparent hover:bg-neutral-600/50"
      />

      <div className="flex h-full min-h-0">
        {showSidebar ? (
          <TerminalSidebar
            activeEnvironmentSlug={currentSession?.target.environmentSlug ?? null}
            activeProjectId={currentSession?.target.projectId ?? null}
            projects={projects}
          />
        ) : null}

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex h-10 shrink-0 items-center gap-1 border-b border-white/10 bg-[#171717] px-2">
            {!currentProject ? (
              <div className="flex min-w-0 flex-1 items-center gap-2 px-2 text-sm text-neutral-400">
                <TerminalIcon className="size-4" />
                <span>Terminal</span>
                <Kbd className="bg-white/10 text-neutral-300">⌘J</Kbd>
              </div>
            ) : (
              <div className="flex min-w-0 flex-1 items-center gap-1">
                {!showSidebar ? (
                  <TerminalPickerPopover
                    align="start"
                    trigger={
                      <button
                        type="button"
                        className="flex h-7 max-w-48 shrink-0 items-center gap-1 rounded-md px-2 text-xs font-medium text-neutral-300 hover:bg-white/5 hover:text-white aria-expanded:bg-white/10 aria-expanded:text-white"
                      >
                        <span className="min-w-0 truncate">
                          {currentProject.projectName}
                          {currentEnvironment?.label ? ` / ${currentEnvironment.label}` : ""}
                        </span>
                        <ChevronDownIcon className="size-3.5 shrink-0 text-neutral-500" />
                      </button>
                    }
                  />
                ) : null}
                {currentEnvironment ? (
                  <TerminalTabStrip
                    activeSessionId={currentSession?.id ?? null}
                    tabs={currentEnvironment.tabs}
                  />
                ) : null}
                <NewShellButton />
              </div>
            )}

            <TerminalPanelMenu />
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    className="text-neutral-300 hover:bg-white/10 hover:text-white"
                    onClick={closePanel}
                    aria-label="Hide terminal"
                  >
                    <XIcon className="size-4" />
                  </Button>
                }
              />
              <TooltipContent>Hide terminal</TooltipContent>
            </Tooltip>
          </div>

          <ShellEnvironmentWarning />

          <div className="relative min-h-0 flex-1 bg-[#101010]">
            {sessions.length === 0 ? (
              <TerminalEmptyState />
            ) : (
              sessions.map((session) => (
                <TerminalInstance
                  key={session.id}
                  active={session.id === currentSession?.id}
                  chunks={outputBySessionId[session.id] ?? []}
                  session={session}
                />
              ))
            )}
            {currentSession ? <TerminalRunEndBar session={currentSession} /> : null}
          </div>
          {currentSession ? <TerminalSharedInputNote session={currentSession} /> : null}
        </div>
      </div>
    </section>
  );
};

function TerminalSidebar({
  activeEnvironmentSlug,
  activeProjectId,
  projects,
}: {
  activeEnvironmentSlug: string | null;
  activeProjectId: string | null;
  projects: TerminalTabProject[];
}) {
  return (
    <aside
      aria-label="Running checkouts"
      className="flex w-46 shrink-0 flex-col border-r border-white/10 bg-[#141414]"
    >
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-white/10 bg-[#171717] pr-2 pl-3">
        <span className="text-xs font-medium text-neutral-400">Running</span>
        <TerminalPickerPopover
          align="start"
          trigger={
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="text-neutral-300 hover:bg-white/10 hover:text-white aria-expanded:bg-white/10 aria-expanded:text-white"
              aria-label="Open a terminal on another project"
            >
              <PlusIcon className="size-4" />
            </Button>
          }
        />
      </div>
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto p-1.5">
        {projects.map((project) => {
          // A project with only its main checkout running is one line, named after the project.
          const single = project.environments.length === 1 && !project.environments[0].label;

          return (
            <div key={project.projectId}>
              {!single ? (
                <div className="truncate px-1.5 pb-0.5 text-[11px] font-medium text-neutral-500">
                  {project.projectName}
                </div>
              ) : null}
              {project.environments.map((environment) => (
                <TerminalSidebarRow
                  key={environment.slug}
                  active={
                    project.projectId === activeProjectId &&
                    environment.slug === activeEnvironmentSlug
                  }
                  environment={environment}
                  label={single ? project.projectName : (environment.label ?? "primary")}
                  projectId={project.projectId}
                />
              ))}
            </div>
          );
        })}
      </div>
    </aside>
  );
}

function TerminalSidebarRow({
  active,
  environment,
  label,
  projectId,
}: {
  active: boolean;
  environment: TerminalTabEnvironment;
  label: string;
  projectId: string;
}) {
  const { selectEnvironment, sessionStates } = useTerminal();
  const states = environment.tabs.map((tab) => sessionStates.get(tab.session.id) ?? "idle");
  const state = states.includes("failed") ? "failed" : states.includes("live") ? "live" : "idle";

  return (
    <button
      type="button"
      aria-current={active ? "true" : undefined}
      onClick={() => selectEnvironment(projectId, environment.slug)}
      className={cn(
        "flex h-7 w-full items-center gap-1.5 rounded-md px-1.5 text-left text-xs transition-colors",
        active
          ? "bg-white/10 text-white"
          : "text-neutral-400 hover:bg-white/5 hover:text-neutral-100",
      )}
    >
      <TerminalStateDot state={state} />
      {environment.label ? (
        <RiGitBranchLine aria-hidden className="size-3 shrink-0 text-neutral-500" />
      ) : null}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span className="shrink-0 font-mono text-[11px] text-neutral-500 tabular-nums">
        {environment.tabs.length}
      </span>
    </button>
  );
}

function TerminalStateDot({ state }: { state: TerminalSessionState }) {
  return (
    <span
      title={state === "failed" ? "Stopped in the background" : undefined}
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        state === "live" && "bg-emerald-400",
        state === "idle" && "bg-neutral-600",
        state === "failed" && "bg-red-500",
      )}
    />
  );
}

function TerminalTabStrip({
  activeSessionId,
  tabs,
}: {
  activeSessionId: string | null;
  tabs: TerminalTabItem[];
}) {
  const { closeSession, setActiveSessionId } = useTerminal();

  return (
    <div className="flex min-w-0 items-center gap-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {tabs.map((tab) => (
        <TerminalTab
          key={tab.session.id}
          active={tab.session.id === activeSessionId}
          tab={tab}
          onClose={() => void closeSession(tab.session.id)}
          onSelect={() => setActiveSessionId(tab.session.id)}
        />
      ))}
    </div>
  );
}

function TerminalTab({
  active,
  onClose,
  onSelect,
  tab,
}: {
  active: boolean;
  onClose: () => void;
  onSelect: () => void;
  tab: TerminalTabItem;
}) {
  const { session, title } = tab;
  const { sessionStates } = useTerminal();
  const buttonRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    if (active) {
      buttonRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }, [active]);

  return (
    <button
      ref={buttonRef}
      type="button"
      onClick={onSelect}
      title={session.workingDirectory}
      className={cn(
        "group/tab flex h-7 max-w-48 shrink-0 items-center gap-1.5 rounded-md pr-0.5 pl-2 text-left text-xs transition-colors",
        active
          ? "bg-white/10 text-white"
          : "text-neutral-400 hover:bg-white/5 hover:text-neutral-100",
      )}
    >
      <TerminalStateDot state={sessionStates.get(session.id) ?? "idle"} />
      <span className="min-w-0 truncate">{title}</span>
      <span
        role="button"
        tabIndex={-1}
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        className={cn(
          "inline-flex size-4 shrink-0 items-center justify-center rounded text-neutral-500 opacity-0 transition-opacity group-hover/tab:opacity-100 hover:bg-white/10 hover:text-white",
          active && "opacity-100",
        )}
        aria-label={`Close ${title}`}
      >
        <XIcon className="size-3" />
      </span>
    </button>
  );
}

function NewShellButton() {
  const { newTabFromActiveTarget } = useTerminal();
  const { hotkeys } = useAppHotkeys();

  const openNewShell = async () => {
    try {
      await newTabFromActiveTarget();
    } catch (error) {
      showTerminalError("Open terminal failed", error);
    }
  };

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="shrink-0 text-neutral-400 hover:bg-white/10 hover:text-white"
            onClick={() => void openNewShell()}
            aria-label="New shell"
          >
            <PlusIcon className="size-4" />
          </Button>
        }
      />
      <TooltipContent>
        New shell
        <Kbd>{formatForDisplay(hotkeys.newTerminalTab)}</Kbd>
      </TooltipContent>
    </Tooltip>
  );
}

function TerminalPickerPopover({
  align,
  trigger,
}: {
  align: "start" | "end";
  trigger: ReactElement<Record<string, unknown>>;
}) {
  const [open, setOpen] = useState(false);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger render={<PopoverTrigger render={trigger} />} />
        <TooltipContent>Open a terminal on another project</TooltipContent>
      </Tooltip>
      <PopoverContent
        align={align}
        collisionAvoidance={{ side: "none" }}
        finalFocus={false}
        className="dark flex max-h-[min(26rem,calc(var(--available-height)-0.5rem))] w-96 flex-col overflow-hidden border border-white/10 bg-[#1b1b1b] text-neutral-100"
      >
        <TerminalPicker onDone={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}

function TerminalEmptyState() {
  return (
    <div className="mx-auto flex h-full min-h-0 max-w-2xl flex-col">
      <TerminalPicker />
    </div>
  );
}

function TerminalPicker({ onDone }: { onDone?: () => void }) {
  const { data } = useQuery(bootstrapQueries.read());
  const { order } = useSharedProjectOrder();
  const [search, setSearch] = useState("");
  const tree = data ? buildTerminalTree(data, order, search) : [];

  return (
    <>
      <div className="shrink-0 border-b border-white/10 p-2">
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-neutral-500" />
          <Input
            autoFocus
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search projects, worktrees, services..."
            className="h-8 border-white/10 bg-white/[0.03] pl-8 text-sm text-neutral-100 placeholder:text-neutral-500 focus-visible:border-white/20 focus-visible:ring-white/10"
          />
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-1.5">
        {tree.length === 0 ? (
          <div className="px-2 py-6 text-center text-sm text-neutral-500">
            No project environment available
          </div>
        ) : (
          tree.map((project) => (
            <div key={project.projectId} className="pb-1">
              <div className="truncate px-2 pt-2 pb-1 text-xs font-medium text-neutral-400">
                {project.projectName}
              </div>
              {project.environments.map((environment) => (
                <TerminalEnvironmentPicker
                  key={environment.environmentSlug}
                  environment={environment}
                  onDone={onDone}
                />
              ))}
            </div>
          ))
        )}
      </div>
    </>
  );
}

function TerminalEnvironmentPicker({
  environment,
  onDone,
}: {
  environment: TerminalTreeEnvironment;
  onDone?: () => void;
}) {
  const { openTerminal } = useTerminal();

  const openShell = async () => {
    onDone?.();
    try {
      await openTerminal(environment.target);
    } catch (error) {
      showTerminalError("Open terminal failed", error);
    }
  };

  return (
    <div>
      {!environment.primary ? (
        <div className="flex h-7 items-center gap-1.5 px-2 font-mono text-[11px] text-neutral-500">
          <RiGitBranchLine aria-hidden className="size-3 shrink-0" />
          <span className="min-w-0 truncate">{environment.environmentSlug}</span>
        </div>
      ) : null}
      <div className={cn(!environment.primary && "ml-3 border-l border-white/10 pl-1")}>
        {environment.services.map((service) => (
          <TerminalServicePicker
            key={`${environment.environmentSlug}-${service.service.id}`}
            onDone={onDone}
            service={service}
          />
        ))}
        <button
          type="button"
          onClick={() => void openShell()}
          className="flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-xs text-neutral-400 hover:bg-white/5 hover:text-neutral-100"
        >
          <TerminalIcon className="size-3.5 shrink-0 text-neutral-500" />
          <span className="min-w-0 truncate">Shell</span>
        </button>
      </div>
    </div>
  );
}

function TerminalServicePicker({
  onDone,
  service,
}: {
  onDone?: () => void;
  service: TerminalTreeService;
}) {
  const { findSessionForTarget, launchService, openTerminal } = useTerminal();
  const hasSession = Boolean(findSessionForTarget(service.target));
  // A service reachable without a tab here runs elsewhere: a shell is all this row can open.
  const canLaunch = !hasSession && !service.reachable;

  const launch = async (profile: string | null = null) => {
    onDone?.();
    try {
      await launchService(service.target, profile);
    } catch (error) {
      showTerminalError("Launch service failed", error);
    }
  };

  const openRow = async () => {
    if (canLaunch) {
      await launch();
      return;
    }

    onDone?.();
    try {
      await openTerminal(service.target);
    } catch (error) {
      showTerminalError("Open terminal failed", error);
    }
  };

  return (
    <div className="flex items-center gap-1">
      <button
        type="button"
        onClick={() => void openRow()}
        aria-label={canLaunch ? `Start ${service.service.name}` : `Open ${service.service.name}`}
        className="group/row flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-xs text-neutral-300 hover:bg-white/5 hover:text-white"
      >
        <span
          className={cn(
            "size-1.5 shrink-0 rounded-full",
            service.reachable ? "bg-emerald-400" : "bg-neutral-600",
          )}
        />
        <span className="shrink-0">{service.service.name}</span>
        {service.domain ? (
          <span className="min-w-0 truncate font-mono text-[11px] text-neutral-600">
            {service.domain}
          </span>
        ) : null}
        {canLaunch ? (
          <PlayIcon className="ml-auto size-3.5 shrink-0 text-neutral-600 group-hover/row:text-neutral-200" />
        ) : null}
      </button>
      {service.launchProfiles.session ? (
        <LaunchProfileBadge session={service.launchProfiles.session} className="dark" />
      ) : null}
      {canLaunch ? (
        <LaunchProfileMenu
          profiles={service.launchProfiles.profiles}
          error={service.launchProfiles.profilesError}
          serviceName={service.service.name}
          onLaunch={(profile) => void launch(profile)}
          className="-ml-1 w-4 text-neutral-500 hover:bg-white/10 hover:text-white aria-expanded:bg-white/10 aria-expanded:text-white"
        />
      ) : null}
    </div>
  );
}

function showTerminalError(title: string, error: unknown) {
  toast.error(title, {
    description: error instanceof Error ? error.message : String(error),
  });
}

interface TerminalTreeProject {
  projectId: string;
  projectName: string;
  environments: TerminalTreeEnvironment[];
}

interface TerminalTreeEnvironment {
  environmentName: string;
  environmentSlug: string;
  primary: boolean;
  services: TerminalTreeService[];
  target: TerminalTargetRequest;
}

interface TerminalTreeService {
  domain: string | null;
  launchProfiles: ServiceLaunchProfiles;
  reachable: boolean;
  service: ProjectServiceInfoDto;
  target: TerminalTargetRequest;
}

function buildTerminalTree(
  data: BootstrapDto,
  projectOrder: string[],
  search: string,
): TerminalTreeProject[] {
  const query = search.trim().toLowerCase();
  const servicesById = new Map(data.services.map((service) => [service.id, service]));
  const domainsByPattern = new Map(
    data.domains.map((domain) => [domain.registration.pattern, domain]),
  );

  const projectsById = new Map(data.projects.map((project) => [project.id, project]));

  return projectOrder
    .flatMap((id) => projectsById.get(id) ?? [])
    .map((project) => {
      const environments: TerminalTreeEnvironment[] = data.projectEnvironments
        .filter((environment) => environment.projectId === project.id && environment.rootPath)
        .sort((left, right) => Number(right.primary) - Number(left.primary))
        .map((environment): TerminalTreeEnvironment => {
          const services: TerminalTreeService[] = [];

          for (const environmentService of environment.services) {
            const service = servicesById.get(environmentService.serviceId);
            if (!service) continue;
            const target = serviceTarget(environment, service);

            services.push({
              domain: environmentService.domain,
              launchProfiles: serviceLaunchProfiles(data, target),
              reachable: domainsByPattern.get(environmentService.domain)?.targetReachable ?? false,
              service,
              target,
            });
          }

          return {
            environmentName: environment.name,
            environmentSlug: environment.slug,
            primary: environment.primary,
            services,
            target:
              environment.primary && project.rootPath
                ? projectTarget(project, environment)
                : environmentTarget(environment),
          } satisfies TerminalTreeEnvironment;
        });

      return {
        projectId: project.id,
        projectName: project.name,
        environments,
      } satisfies TerminalTreeProject;
    })
    .filter((project) => project.environments.length > 0)
    .map((project) => {
      if (!query) return project;

      const projectMatches = project.projectName.toLowerCase().includes(query);
      const environments = project.environments
        .map((environment) => {
          const environmentMatches =
            environment.environmentName.toLowerCase().includes(query) ||
            environment.environmentSlug.toLowerCase().includes(query);
          const services = environment.services.filter(
            (service) =>
              projectMatches ||
              environmentMatches ||
              service.service.name.toLowerCase().includes(query) ||
              service.service.primaryDomain.toLowerCase().includes(query) ||
              service.domain?.toLowerCase().includes(query),
          );
          if (projectMatches || environmentMatches || services.length > 0) {
            return {
              ...environment,
              services: projectMatches || environmentMatches ? environment.services : services,
            };
          }
          return null;
        })
        .filter((environment): environment is TerminalTreeEnvironment => Boolean(environment));

      return { ...project, environments };
    })
    .filter((project) => project.environments.length > 0);
}

function environmentTarget(environment: ProjectEnvironmentInfoDto): TerminalTargetRequest {
  return {
    kind: "projectEnvironment",
    projectId: environment.projectId,
    environmentSlug: environment.slug,
  };
}

function projectTarget(
  project: ProjectInfoDto,
  environment: ProjectEnvironmentInfoDto,
): TerminalTargetRequest {
  return {
    kind: "project",
    projectId: project.id,
    environmentSlug: environment.slug,
  };
}

function serviceTarget(
  environment: ProjectEnvironmentInfoDto,
  service: ProjectServiceInfoDto,
): TerminalTargetRequest {
  return {
    kind: "service",
    projectId: environment.projectId,
    environmentSlug: environment.slug,
    serviceId: service.id,
  };
}
