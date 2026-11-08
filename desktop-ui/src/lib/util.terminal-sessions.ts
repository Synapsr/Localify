import type {
  DomainInfoDto,
  ProjectEnvironmentInfoDto,
  TerminalSessionDto,
} from "@/types/localify";

/** `failed` is a service that stopped while its tab was in the background. */
export type TerminalSessionState = "live" | "idle" | "failed";

export interface TerminalTabItem {
  session: TerminalSessionDto;
  title: string;
}

export interface TerminalTabEnvironment {
  /** Null for the project's primary checkout, which needs no label. */
  label: string | null;
  slug: string;
  tabs: TerminalTabItem[];
}

export interface TerminalTabProject {
  projectId: string;
  projectName: string;
  environments: TerminalTabEnvironment[];
}

/**
 * Same order as the projects screen: projects as the user arranged them, the primary
 * checkout before its worktrees, services before plain shells.
 */
export function sortTerminalSessions(
  sessions: TerminalSessionDto[],
  projectOrder: string[],
  environments: ProjectEnvironmentInfoDto[],
): TerminalSessionDto[] {
  const projectRank = new Map(projectOrder.map((id, index) => [id, index]));
  const environmentRank = new Map(
    environments.map((environment, index) => [
      environmentKey(environment.projectId, environment.slug),
      environment.primary ? -1 : index,
    ]),
  );
  const serviceRank = new Map(
    environments.map((environment) => [
      environmentKey(environment.projectId, environment.slug),
      orderedServiceIds(environment),
    ]),
  );

  const rankOf = (session: TerminalSessionDto) => {
    const { environmentSlug, projectId, serviceId } = session.target;
    const key = environmentKey(projectId, environmentSlug);
    const service =
      session.run && serviceId ? (serviceRank.get(key)?.indexOf(serviceId) ?? -1) : -1;

    return [
      projectRank.get(projectId) ?? Number.MAX_SAFE_INTEGER,
      environmentRank.get(key) ?? Number.MAX_SAFE_INTEGER,
      !session.run
        ? Number.MAX_SAFE_INTEGER
        : service === -1
          ? Number.MAX_SAFE_INTEGER - 1
          : service,
    ];
  };

  return [...sessions].sort((left, right) => {
    const leftRank = rankOf(left);
    const rightRank = rankOf(right);

    return (
      leftRank[0] - rightRank[0] ||
      left.target.projectName.localeCompare(right.target.projectName) ||
      leftRank[1] - rightRank[1] ||
      left.target.environmentSlug.localeCompare(right.target.environmentSlug) ||
      leftRank[2] - rightRank[2]
    );
  });
}

/** Groups sessions already sorted by `sortTerminalSessions` by project, then checkout. */
export function groupTerminalSessions(sessions: TerminalSessionDto[]): TerminalTabProject[] {
  const projects: TerminalTabProject[] = [];

  for (const session of sessions) {
    const { environmentSlug, projectId, projectName } = session.target;

    let project = projects[projects.length - 1];
    if (project?.projectId !== projectId) {
      project = { projectId, projectName, environments: [] };
      projects.push(project);
    }

    let environment = project.environments[project.environments.length - 1];
    if (environment?.slug !== environmentSlug) {
      environment = {
        label: environmentSlug === "primary" ? null : environmentSlug,
        slug: environmentSlug,
        tabs: [],
      };
      project.environments.push(environment);
    }

    const baseTitle = tabTitle(session);
    const sameTitleCount = environment.tabs.filter(
      (tab) => tabTitle(tab.session) === baseTitle,
    ).length;
    environment.tabs.push({
      session,
      title: sameTitleCount === 0 ? baseTitle : `${baseTitle} ${sameTitleCount + 1}`,
    });
  }

  return projects;
}

// A service tab is named after its service; a shell, even in a service's folder, is a shell.
function tabTitle(session: TerminalSessionDto) {
  return session.run ? (session.target.serviceName ?? "Service") : "Shell";
}

/** Service tabs whose domain answers: a run can be up before its port is. */
export function reachableServiceSessionIds(
  sessions: TerminalSessionDto[],
  environments: ProjectEnvironmentInfoDto[],
  domains: DomainInfoDto[],
) {
  const reachable = new Set<string>();

  for (const session of sessions) {
    const { environmentSlug, projectId, serviceId } = session.target;
    if (!session.run || !serviceId) continue;

    const pattern = environments
      .find(
        (environment) =>
          environment.projectId === projectId && environment.slug === environmentSlug,
      )
      ?.services.find((service) => service.serviceId === serviceId)?.domain;
    const domain = domains.find((candidate) => candidate.registration.pattern === pattern);
    if (domain?.targetReachable) {
      reachable.add(session.id);
    }
  }

  return reachable;
}

function environmentKey(projectId: string, slug: string) {
  return `${projectId}/${slug}`;
}

// The projects screen lists the primary checkout by domain, and a worktree's services as they come.
function orderedServiceIds(environment: ProjectEnvironmentInfoDto) {
  const services = environment.primary
    ? [...environment.services].sort((left, right) => left.domain.localeCompare(right.domain))
    : environment.services;
  return services.map((service) => service.serviceId);
}
