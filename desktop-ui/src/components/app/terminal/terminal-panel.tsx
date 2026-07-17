"use client";

import { useQuery } from "@tanstack/react-query";
import { useState, type PointerEvent as ReactPointerEvent } from "react";
import { toast } from "sonner";

import type {
  BootstrapDto,
  ProjectEnvironmentInfoDto,
  ProjectInfoDto,
  ProjectServiceInfoDto,
  TerminalSessionDto,
  TerminalSessionTargetDto,
  TerminalTargetRequest,
} from "@/types/localify";

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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { bootstrapQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { toTerminalTargetRequest, useTerminal } from "@/providers/terminal-provider";

import { TerminalInstance } from "./terminal-instance";

const SIDEBAR_WIDTH = 280;

export const TerminalPanel = () => {
  const {
    activeSession,
    closePanel,
    closeSession,
    isOpen,
    newTabFromActiveTarget,
    outputBySessionId,
    panelHeight,
    selectedTarget,
    sessions,
    setActiveSessionId,
    setPanelHeight,
    sidebarCollapsed,
    toggleSidebar,
  } = useTerminal();

  if (!isOpen) {
    return null;
  }

  const activeEnvironmentTarget =
    selectedTarget ?? (activeSession ? toTerminalTargetRequest(activeSession.target) : null);
  const scopedSessions = activeEnvironmentTarget
    ? sessions.filter((session) =>
        sessionMatchesEnvironment(session.target, activeEnvironmentTarget),
      )
    : sessions;
  const scopedActiveSession =
    activeSession && scopedSessions.some((session) => session.id === activeSession.id)
      ? activeSession
      : (scopedSessions[0] ?? null);
  const scopedActiveSessionId = scopedActiveSession?.id ?? null;
  const scopedActiveSessionMeta = scopedActiveSession
    ? terminalSessionMeta(scopedActiveSession)
    : null;

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

      <div className="flex h-full min-h-0 flex-col">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b border-white/10 bg-[#171717] px-2">
          <div className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
            {scopedSessions.length === 0 ? (
              <div className="flex items-center gap-2 px-2 text-sm text-neutral-400">
                <TerminalIcon className="size-4" />
                <span>Terminal</span>
                <Kbd>⌘J</Kbd>
              </div>
            ) : (
              scopedSessions.map((session) => (
                <TerminalTab
                  key={session.id}
                  active={session.id === scopedActiveSessionId}
                  session={session}
                  onClose={() => void closeSession(session.id)}
                  onSelect={() => setActiveSessionId(session.id)}
                />
              ))
            )}
          </div>

          {scopedActiveSessionMeta ? (
            <div className="hidden max-w-[45%] min-w-0 shrink items-center gap-1.5 border-l border-white/10 pl-3 text-[11px] text-neutral-500 lg:flex">
              <span className="shrink-0 text-neutral-300">{scopedActiveSessionMeta.label}</span>
              <span className="text-neutral-700">/</span>
              <span
                className="min-w-0 truncate font-mono"
                title={scopedActiveSession.workingDirectory}
              >
                {scopedActiveSessionMeta.path}
              </span>
            </div>
          ) : null}

          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="text-neutral-300 hover:bg-white/10 hover:text-white"
                  onClick={() => void newTabFromActiveTarget()}
                  aria-label="New terminal tab"
                >
                  <PlusIcon className="size-4" />
                </Button>
              }
            />
            <TooltipContent>New tab</TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  className="text-neutral-300 hover:bg-white/10 hover:text-white"
                  onClick={toggleSidebar}
                  aria-label={sidebarCollapsed ? "Show terminal sidebar" : "Hide terminal sidebar"}
                >
                  <ChevronDownIcon
                    className={cn(
                      "size-4 transition-transform",
                      sidebarCollapsed ? "rotate-90" : "-rotate-90",
                    )}
                  />
                </Button>
              }
            />
            <TooltipContent>{sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}</TooltipContent>
          </Tooltip>

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

        <div className="flex min-h-0 flex-1">
          <div className="relative min-w-0 flex-1 bg-[#101010]">
            {scopedSessions.length === 0 ? (
              <TerminalEmptyState />
            ) : (
              scopedSessions.map((session) => (
                <TerminalInstance
                  key={session.id}
                  active={session.id === scopedActiveSessionId}
                  chunks={outputBySessionId[session.id] ?? []}
                  session={session}
                />
              ))
            )}
          </div>

          {!sidebarCollapsed ? (
            <TerminalSidebar
              activeEnvironmentTarget={activeEnvironmentTarget}
              sessions={sessions}
            />
          ) : null}
        </div>
      </div>
    </section>
  );
};

function TerminalTab({
  active,
  onClose,
  onSelect,
  session,
}: {
  active: boolean;
  onClose: () => void;
  onSelect: () => void;
  session: TerminalSessionDto;
}) {
  const title = sessionDisplayTitle(session);

  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "group/tab flex h-8 max-w-48 shrink-0 items-center gap-1.5 rounded-md px-2 text-left text-xs transition-colors",
        active
          ? "bg-white/10 text-white"
          : "text-neutral-400 hover:bg-white/5 hover:text-neutral-100",
      )}
    >
      <span
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          session.status === "running" ? "bg-emerald-400" : "bg-neutral-600",
        )}
      />
      <span className="min-w-0 truncate">{title}</span>
      <span
        role="button"
        tabIndex={-1}
        onClick={(event) => {
          event.stopPropagation();
          onClose();
        }}
        className="ml-1 inline-flex size-5 shrink-0 items-center justify-center rounded text-neutral-500 opacity-0 transition-opacity group-hover/tab:opacity-100 hover:bg-white/10 hover:text-white"
        aria-label={`Close ${title}`}
      >
        <XIcon className="size-3" />
      </span>
    </button>
  );
}

function TerminalSidebar({
  activeEnvironmentTarget,
  sessions,
}: {
  activeEnvironmentTarget: TerminalTargetRequest | null;
  sessions: TerminalSessionDto[];
}) {
  const { activeSessionId, openTerminal, setActiveSessionId } = useTerminal();
  const { data } = useQuery(bootstrapQueries.read());
  const tree = buildTerminalSidebarTree(data, sessions, activeEnvironmentTarget);

  const selectEnvironment = async (environment: TerminalTreeEnvironment) => {
    const firstSession = sidebarEnvironmentSession(environment) ?? environment.sessions[0] ?? null;
    if (firstSession) {
      setActiveSessionId(firstSession.id);
      return;
    }

    try {
      await openTerminal(environment.target);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <aside
      className="min-h-0 shrink-0 overflow-y-auto border-l border-white/10 bg-[#151515] p-2"
      style={{ width: SIDEBAR_WIDTH }}
    >
      <div className="mb-2 px-1 text-xs font-medium tracking-wide text-neutral-500 uppercase">
        Sessions
      </div>
      {tree.map((project) => (
        <div key={project.projectId} className="mb-3">
          <div className="truncate px-1 text-sm font-medium text-neutral-200">
            {project.projectName}
          </div>
          <div className="mt-1 space-y-1">
            {project.environments.map((environment) => {
              const visibleSessions = sidebarVisibleSessions(environment);

              return (
                <div key={environment.environmentSlug}>
                  <button
                    type="button"
                    onClick={() => void selectEnvironment(environment)}
                    className={cn(
                      "flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-xs transition-colors",
                      !environment.primary && project.environments.length > 1
                        ? "ml-3 w-[calc(100%-0.75rem)] border-l border-white/10 pl-3"
                        : "",
                      targetEnvironmentMatches(activeEnvironmentTarget, environment.target)
                        ? "bg-white/10 text-white"
                        : "text-neutral-400 hover:bg-white/5 hover:text-neutral-100",
                    )}
                  >
                    <span
                      className={cn(
                        "size-1.5 shrink-0 rounded-full",
                        environment.sessions.some((session) => session.status === "running")
                          ? "bg-emerald-400"
                          : "bg-neutral-600",
                      )}
                    />
                    <span className="min-w-0 flex-1 truncate">{environment.environmentLabel}</span>
                    {environment.sessions.length > 0 ? (
                      <span className="text-[11px] text-neutral-500">
                        {environment.sessions.length}
                      </span>
                    ) : null}
                  </button>
                  {visibleSessions.length > 0 ? (
                    <div
                      className={cn(
                        "mt-1 space-y-1",
                        !environment.primary && project.environments.length > 1
                          ? "ml-6 border-l border-white/10 pl-2"
                          : "ml-3",
                      )}
                    >
                      {visibleSessions.map((session) => (
                        <button
                          key={session.id}
                          type="button"
                          onClick={() => setActiveSessionId(session.id)}
                          className={cn(
                            "flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-xs transition-colors",
                            session.id === activeSessionId
                              ? "bg-white/10 text-white"
                              : "text-neutral-400 hover:bg-white/5 hover:text-neutral-100",
                          )}
                        >
                          <span
                            className={cn(
                              "size-1.5 shrink-0 rounded-full",
                              session.status === "running" ? "bg-emerald-400" : "bg-neutral-600",
                            )}
                          />
                          <span className="min-w-0 truncate">{sessionDisplayTitle(session)}</span>
                        </button>
                      ))}
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </aside>
  );
}

function TerminalEmptyState() {
  const { data } = useQuery(bootstrapQueries.read());
  const [search, setSearch] = useState("");
  const tree = data ? buildTerminalTree(data, search) : [];

  return (
    <div className="flex h-full min-h-0 flex-col bg-[#111111]">
      <div className="border-b border-white/10 p-3">
        <div className="relative">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-neutral-500" />
          <Input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search projects, environments, services..."
            className="h-9 border-white/10 bg-white/[0.03] pl-8 text-sm text-neutral-100 placeholder:text-neutral-500 focus-visible:border-white/20 focus-visible:ring-white/10"
          />
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {tree.length === 0 ? (
          <div className="flex h-full items-center justify-center text-sm text-neutral-500">
            No project environment available
          </div>
        ) : (
          <div className="mx-auto max-w-2xl space-y-3">
            {tree.map((project) => (
              <TerminalProjectPicker key={project.projectId} project={project} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function TerminalProjectPicker({ project }: { project: TerminalTreeProject }) {
  return (
    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2">
      <div className="px-1 pb-1 text-sm font-medium text-neutral-100">{project.projectName}</div>
      <div className="space-y-1">
        {project.environments.map((environment) => (
          <TerminalEnvironmentPicker key={environment.environmentSlug} environment={environment} />
        ))}
      </div>
    </div>
  );
}

function TerminalEnvironmentPicker({ environment }: { environment: TerminalTreeEnvironment }) {
  const { launchService, openTerminal } = useTerminal();

  const openEnvironment = async () => {
    try {
      await openTerminal(environment.target);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <div className="rounded-md">
      <button
        type="button"
        onClick={() => void openEnvironment()}
        className="flex h-9 w-full items-center gap-2 rounded-md px-2 text-left text-sm text-neutral-300 hover:bg-white/5 hover:text-white"
      >
        <TerminalIcon className="size-4 shrink-0 text-neutral-500" />
        <span className="min-w-0 flex-1 truncate">{environment.environmentLabel}</span>
      </button>
      {environment.services.length > 0 ? (
        <div className="ml-4 border-l border-white/10 pl-2">
          {environment.services.map((service) => (
            <TerminalServicePicker
              key={`${environment.environmentSlug}-${service.service.id}`}
              launchService={launchService}
              service={service}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function TerminalServicePicker({
  launchService,
  service,
}: {
  launchService: (target: TerminalTargetRequest) => Promise<TerminalSessionDto>;
  service: TerminalTreeService;
}) {
  const { openTerminal } = useTerminal();

  const openService = async () => {
    try {
      await openTerminal(service.target);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const startService = async () => {
    try {
      await launchService(service.target);
    } catch (error) {
      toast.error("Launch service failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <div className="flex items-center gap-1 py-0.5">
      <button
        type="button"
        onClick={() => void openService()}
        className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-2 text-left text-xs text-neutral-400 hover:bg-white/5 hover:text-neutral-100"
      >
        <span
          className={cn(
            "size-1.5 shrink-0 rounded-full",
            service.reachable ? "bg-emerald-400" : "bg-neutral-600",
          )}
        />
        <span className="min-w-0 truncate">{service.service.name}</span>
        {service.domain ? (
          <span className="hidden min-w-0 truncate font-mono text-neutral-600 sm:inline">
            {service.domain}
          </span>
        ) : null}
      </button>
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              className="text-neutral-500 hover:bg-white/10 hover:text-white"
              onClick={() => void startService()}
              disabled={service.reachable}
              aria-label={`Launch ${service.service.name}`}
            >
              <PlayIcon className="size-3.5" />
            </Button>
          }
        />
        <TooltipContent>
          {service.reachable ? "Service already reachable" : "Launch service"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

interface TerminalTreeProject {
  projectId: string;
  projectName: string;
  environments: TerminalTreeEnvironment[];
}

interface TerminalTreeEnvironment {
  environmentLabel: string;
  environmentName: string;
  environmentSlug: string;
  primary: boolean;
  services: TerminalTreeService[];
  sessions: TerminalSessionDto[];
  target: TerminalTargetRequest;
}

interface TerminalTreeService {
  domain: string | null;
  reachable: boolean;
  service: ProjectServiceInfoDto;
  target: TerminalTargetRequest;
}

function buildTerminalTree(data: BootstrapDto, search: string): TerminalTreeProject[] {
  const query = search.trim().toLowerCase();
  const servicesById = new Map(data.services.map((service) => [service.id, service]));
  const domainsByPattern = new Map(
    data.domains.map((domain) => [domain.registration.pattern, domain]),
  );

  return data.projects
    .map((project) => {
      const environments: TerminalTreeEnvironment[] = data.projectEnvironments
        .filter((environment) => environment.projectId === project.id && environment.rootPath)
        .sort(compareProjectEnvironments)
        .map((environment): TerminalTreeEnvironment => {
          const services: TerminalTreeService[] = [];

          for (const environmentService of environment.services) {
            const service = servicesById.get(environmentService.serviceId);
            if (!service) continue;

            services.push({
              domain: environmentService.domain,
              reachable: domainsByPattern.get(environmentService.domain)?.targetReachable ?? false,
              service,
              target: serviceTarget(environment, service),
            });
          }

          return {
            environmentLabel: environmentDisplayLabel(project, environment),
            environmentName: environment.name,
            environmentSlug: environment.slug,
            primary: environment.primary,
            services,
            sessions: [],
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

function buildTerminalSidebarTree(
  data: BootstrapDto | undefined,
  sessions: TerminalSessionDto[],
  activeEnvironmentTarget: TerminalTargetRequest | null,
): TerminalTreeProject[] {
  const projects = new Map<string, TerminalTreeProject>();

  if (data) {
    for (const project of buildTerminalTree(data, "")) {
      projects.set(project.projectId, {
        ...project,
        environments: project.environments.map((environment) => ({
          ...environment,
          sessions: [],
        })),
      });
    }
  }

  for (const session of sessions) {
    let project = projects.get(session.target.projectId);
    if (!project) {
      project = {
        projectId: session.target.projectId,
        projectName: session.target.projectName,
        environments: [],
      };
      projects.set(session.target.projectId, project);
    }

    let environment = project.environments.find(
      (candidate) => candidate.environmentSlug === session.target.environmentSlug,
    );
    if (!environment) {
      environment = {
        environmentLabel: sessionEnvironmentLabel(session.target) ?? session.target.projectName,
        environmentName: session.target.environmentName,
        environmentSlug: session.target.environmentSlug,
        primary: isPrimaryEnvironment(
          session.target.environmentSlug,
          session.target.environmentName,
        ),
        services: [],
        sessions: [],
        target: toTerminalTargetRequest(session.target),
      };
      project.environments.push(environment);
    }

    environment.sessions.push(session);
  }

  return sortTreeProjects(Array.from(projects.values()), activeEnvironmentTarget)
    .map((project) => ({
      ...project,
      environments: sortTreeEnvironments(project.environments, activeEnvironmentTarget),
    }))
    .filter((project) => project.environments.length > 0);
}

function sessionDisplayTitle(session: TerminalSessionDto) {
  if (session.target.serviceName) {
    return session.target.serviceName;
  }

  if (session.target.kind === "project") {
    return session.target.projectName;
  }

  return sessionEnvironmentLabel(session.target) ?? session.target.projectName;
}

function terminalSessionMeta(session: TerminalSessionDto) {
  const labels = [session.target.projectName];
  const environmentLabel = sessionEnvironmentLabel(session.target);

  if (environmentLabel) {
    labels.push(environmentLabel);
  }

  if (session.target.serviceName) {
    labels.push(session.target.serviceName);
  }

  return {
    label: labels.join(" / "),
    path: compactWorkingDirectory(session.workingDirectory),
  };
}

function sessionEnvironmentLabel(target: TerminalSessionTargetDto) {
  return isPrimaryEnvironment(target.environmentSlug, target.environmentName)
    ? null
    : target.environmentName;
}

function environmentDisplayLabel(project: ProjectInfoDto, environment: ProjectEnvironmentInfoDto) {
  return environment.primary ? project.name : environment.name;
}

function sidebarVisibleSessions(environment: TerminalTreeEnvironment) {
  return environment.sessions.filter(
    (session) => !isSidebarEnvironmentSession(session, environment),
  );
}

function sidebarEnvironmentSession(environment: TerminalTreeEnvironment) {
  return (
    environment.sessions.find((session) => isSidebarEnvironmentSession(session, environment)) ??
    null
  );
}

function isSidebarEnvironmentSession(
  session: TerminalSessionDto,
  environment: TerminalTreeEnvironment,
) {
  return (
    session.target.kind !== "service" &&
    normalizedLabel(sessionDisplayTitle(session)) === normalizedLabel(environment.environmentLabel)
  );
}

function sessionMatchesEnvironment(
  sessionTarget: TerminalSessionTargetDto,
  target: TerminalTargetRequest,
) {
  return (
    sessionTarget.projectId === target.projectId &&
    sessionTarget.environmentSlug === target.environmentSlug
  );
}

function targetEnvironmentMatches(
  left: TerminalTargetRequest | null,
  right: TerminalTargetRequest | null,
) {
  if (!left || !right) {
    return false;
  }

  return left.projectId === right.projectId && left.environmentSlug === right.environmentSlug;
}

function compareProjectEnvironments(
  left: ProjectEnvironmentInfoDto,
  right: ProjectEnvironmentInfoDto,
) {
  if (left.primary !== right.primary) {
    return left.primary ? -1 : 1;
  }

  return left.name.localeCompare(right.name);
}

function sortTreeProjects(
  projects: TerminalTreeProject[],
  activeEnvironmentTarget: TerminalTargetRequest | null,
) {
  return [...projects].sort((left, right) => {
    const leftSelected = projectMatchesTarget(left, activeEnvironmentTarget);
    const rightSelected = projectMatchesTarget(right, activeEnvironmentTarget);
    if (leftSelected !== rightSelected) {
      return leftSelected ? -1 : 1;
    }

    const leftActive = projectHasSessions(left);
    const rightActive = projectHasSessions(right);
    if (leftActive !== rightActive) {
      return leftActive ? -1 : 1;
    }

    return left.projectName.localeCompare(right.projectName);
  });
}

function sortTreeEnvironments(
  environments: TerminalTreeEnvironment[],
  activeEnvironmentTarget: TerminalTargetRequest | null,
) {
  return [...environments].sort((left, right) => {
    const leftSelected = targetEnvironmentMatches(activeEnvironmentTarget, left.target);
    const rightSelected = targetEnvironmentMatches(activeEnvironmentTarget, right.target);
    if (leftSelected !== rightSelected) {
      return leftSelected ? -1 : 1;
    }

    const leftActive = left.sessions.length > 0;
    const rightActive = right.sessions.length > 0;
    if (leftActive !== rightActive) {
      return leftActive ? -1 : 1;
    }

    if (left.primary !== right.primary) {
      return left.primary ? -1 : 1;
    }

    return left.environmentLabel.localeCompare(right.environmentLabel);
  });
}

function projectMatchesTarget(project: TerminalTreeProject, target: TerminalTargetRequest | null) {
  return Boolean(target && project.projectId === target.projectId);
}

function projectHasSessions(project: TerminalTreeProject) {
  return project.environments.some((environment) => environment.sessions.length > 0);
}

function normalizedLabel(value: string) {
  return value.trim().toLowerCase();
}

function compactWorkingDirectory(path: string) {
  return path.replace(/^\/Users\/[^/]+/, "~").replace(/^\/home\/[^/]+/, "~");
}

function isPrimaryEnvironment(slug: string, name: string) {
  return slug === "primary" || name.toLowerCase() === "primary";
}
