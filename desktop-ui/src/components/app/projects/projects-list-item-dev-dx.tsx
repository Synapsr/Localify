"use client";

import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";

import type {
  DomainInfoDto,
  ProjectEnvironmentInfoDto,
  ProjectServiceInfoDto,
  TerminalTargetRequest,
} from "@/types/localify";

import { ChevronDownIcon } from "@/components/icons";
import { cn } from "@/lib/utils";
import { terminalTargetRequestMatches, useTerminal } from "@/providers/terminal-provider";

import { ProjectDomainChip } from "./projects-list-item-domain-chip";
import { ProjectsListItemEnvironmentActions } from "./projects-list-item-environment-actions";

interface ProjectsListItemDevDxProps {
  domains: DomainInfoDto[];
  services: ProjectServiceInfoDto[];
  environments: ProjectEnvironmentInfoDto[];
  projectRootPath: string | null;
}

const chipVariants = {
  initial: { opacity: 0, y: -4 },
  animate: { opacity: 1, y: 0 },
};

export const ProjectsListItemDevDx = ({
  domains,
  services,
  environments,
  projectRootPath,
}: ProjectsListItemDevDxProps) => {
  const { selectedTarget, setSelectedTarget } = useTerminal();
  const [openEnvironmentSlugs, setOpenEnvironmentSlugs] = useState<Set<string>>(() => new Set());
  const visibleEnvironments = environments.filter((environment) => !environment.primary);
  const domainByPattern = new Map(domains.map((domain) => [domain.registration.pattern, domain]));
  const serviceById = new Map(services.map((service) => [service.id, service]));

  if (visibleEnvironments.length === 0) {
    return null;
  }

  const toggleEnvironment = (slug: string) => {
    setOpenEnvironmentSlugs((current) => {
      const next = new Set(current);
      if (next.has(slug)) {
        next.delete(slug);
      } else {
        next.add(slug);
      }
      return next;
    });
  };

  return (
    <div className="mt-1.5 flex flex-col gap-0.5 border-t border-border/60 pt-1.5">
      {visibleEnvironments.map((environment) => {
        const open = openEnvironmentSlugs.has(environment.slug);
        const routesId = `project-environment-routes-${environment.slug}`;
        const serviceCount = environment.services.length;
        const reachableCount = environment.services.filter((service) => {
          const domain = domainByPattern.get(service.domain);
          return domain?.targetReachable;
        }).length;
        const allReachable = serviceCount > 0 && reachableCount === serviceCount;
        const someReachable = reachableCount > 0;
        const worktree = isWorktreeEnvironment(environment, projectRootPath);
        const branchLabel = visibleBranchLabel(environment);
        const showServiceLabels = serviceCount > 1;
        const target = environmentTarget(environment);
        const selected = terminalTargetRequestMatches(selectedTarget, target);

        return (
          <div key={environment.slug}>
            <div
              className={cn(
                "group/env flex w-full items-center gap-1 rounded-md py-1",
                selected && "bg-muted/60",
              )}
            >
              <button
                type="button"
                className="flex min-w-0 flex-1 items-center gap-2 rounded-md text-left transition-transform duration-150 outline-none focus-visible:ring-2 focus-visible:ring-ring/50 active:translate-y-px"
                aria-expanded={open}
                aria-controls={routesId}
                onClick={() => {
                  if (target) {
                    setSelectedTarget(target);
                  }
                  toggleEnvironment(environment.slug);
                }}
              >
                <ChevronDownIcon
                  className={cn(
                    "size-3.5 shrink-0 text-muted-foreground/60 transition-[transform,color] duration-200 ease-out group-hover/env:text-foreground",
                    !open && "-rotate-90",
                  )}
                />
                <span
                  className={cn(
                    "size-1.5 shrink-0 rounded-full transition-colors",
                    allReachable ? "bg-success" : someReachable ? "bg-amber-500" : "bg-neutral-400",
                  )}
                  aria-hidden
                />
                {worktree ? (
                  <span className="shrink-0 rounded-full border border-border/60 bg-muted/60 px-1.5 py-px text-[10px] font-medium text-muted-foreground">
                    worktree
                  </span>
                ) : null}
                <span className="min-w-0 truncate text-sm font-medium text-foreground">
                  {environment.slug}
                </span>
                {branchLabel ? (
                  <span className="hidden min-w-0 truncate text-xs text-muted-foreground sm:inline">
                    {branchLabel}
                  </span>
                ) : null}
                <span className="ml-auto shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
                  {reachableCount}/{serviceCount}
                </span>
              </button>
              <ProjectsListItemEnvironmentActions
                environment={environment}
                terminalTarget={target}
              />
            </div>

            <AnimatePresence initial={false}>
              {open ? (
                <motion.div
                  key="routes"
                  id={routesId}
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: "auto", opacity: 1 }}
                  exit={{ height: 0, opacity: 0 }}
                  transition={{ duration: 0.22, ease: [0.2, 0, 0, 1] }}
                  className="overflow-hidden"
                >
                  <div className="ml-[7px] flex flex-col gap-2 border-l border-border/60 py-1.5 text-muted-foreground">
                    {environment.services.map((service, index) => {
                      const domain = domainByPattern.get(service.domain);
                      const serviceLabel =
                        serviceById.get(service.serviceId)?.name ?? service.serviceId;
                      const extraRoutes =
                        domain?.registration.routes.filter((route) => route.path !== "/") ?? [];

                      return (
                        <motion.div
                          key={`${environment.slug}-${service.serviceId}-${service.domain}`}
                          variants={chipVariants}
                          initial="initial"
                          animate="animate"
                          transition={{
                            type: "spring",
                            stiffness: 500,
                            damping: 35,
                            mass: 0.6,
                            delay: index * 0.04,
                          }}
                          className="relative flex min-w-0 flex-col gap-1 pl-3"
                        >
                          <span
                            className="absolute top-4 left-0 h-px w-2.5 -translate-y-1/2 bg-border/60"
                            aria-hidden
                          />
                          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                            {showServiceLabels ? (
                              <span className="shrink-0 text-sm text-foreground">
                                {serviceLabel}
                              </span>
                            ) : null}
                            {domain ? (
                              <ProjectDomainChip
                                domain={domain}
                                service={serviceById.get(service.serviceId) ?? null}
                                terminalTarget={
                                  environment.rootPath
                                    ? {
                                        kind: "service",
                                        projectId: environment.projectId,
                                        environmentSlug: environment.slug,
                                        serviceId: service.serviceId,
                                      }
                                    : null
                                }
                              />
                            ) : (
                              <span className="min-w-0 truncate font-mono text-xs">
                                {service.domain}
                              </span>
                            )}
                          </div>
                          {extraRoutes.length > 0 ? (
                            <div className="pl-2 font-mono text-xs">
                              {extraRoutes.map((route) => (
                                <span
                                  key={`${route.path}-${route.target}`}
                                  className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-1"
                                  title={`${route.path} -> ${route.target}`}
                                >
                                  <span className="text-muted-foreground/70">{route.path}</span>
                                  <span className="min-w-0 truncate">-&gt; {route.target}</span>
                                </span>
                              ))}
                            </div>
                          ) : null}
                        </motion.div>
                      );
                    })}
                  </div>
                </motion.div>
              ) : null}
            </AnimatePresence>
          </div>
        );
      })}
    </div>
  );
};

const normalizePath = (path: string | null) => {
  return path?.replace(/\/+$/, "") ?? null;
};

const environmentTarget = (
  environment: ProjectEnvironmentInfoDto,
): TerminalTargetRequest | null => {
  if (!environment.rootPath) {
    return null;
  }

  return {
    kind: "projectEnvironment",
    projectId: environment.projectId,
    environmentSlug: environment.slug,
  };
};

const isWorktreeEnvironment = (
  environment: ProjectEnvironmentInfoDto,
  projectRootPath: string | null,
) => {
  const environmentRoot = normalizePath(environment.rootPath);
  const projectRoot = normalizePath(projectRootPath);

  if (!environmentRoot) {
    return false;
  }

  if (projectRoot) {
    return environmentRoot !== projectRoot && Boolean(environment.gitCommonDir);
  }

  return Boolean(environment.gitCommonDir && environment.gitBranch);
};

const normalizeLabel = (value: string | null) => {
  return (
    value
      ?.trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") ?? null
  );
};

const visibleBranchLabel = (environment: ProjectEnvironmentInfoDto) => {
  const branch = environment.gitBranch?.trim();
  if (!branch) {
    return null;
  }

  return normalizeLabel(branch) === normalizeLabel(environment.slug) ? null : branch;
};
