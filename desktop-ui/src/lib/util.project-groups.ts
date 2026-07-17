import type { DomainInfoDto, ProjectInfoDto } from "../types/localify";

export interface ProjectGroup {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
}

export function buildProjectGroups(
  projects: ProjectInfoDto[],
  domains: DomainInfoDto[],
): ProjectGroup[] {
  const domainsByProject = new Map<string, DomainInfoDto[]>();

  for (const domain of domains) {
    if (!domain.projectId) {
      continue;
    }

    const existing = domainsByProject.get(domain.projectId) ?? [];
    existing.push(domain);
    domainsByProject.set(domain.projectId, existing);
  }

  return projects.map((project) => ({
    project,
    domains: (domainsByProject.get(project.id) ?? []).sort((left, right) =>
      left.registration.pattern.localeCompare(right.registration.pattern),
    ),
  }));
}

export function listUnassignedDomains(domains: DomainInfoDto[]): DomainInfoDto[] {
  return domains
    .filter((domain) => domain.projectId === null)
    .sort((left, right) =>
      left.registration.pattern.localeCompare(right.registration.pattern),
    );
}
