import type { ProjectGroup } from "@/lib/util.project-groups";

function includesQuery(value: string, normalizedQuery: string): boolean {
  return value.toLocaleLowerCase().includes(normalizedQuery);
}

function matchesProjectGroup(group: ProjectGroup, normalizedQuery: string): boolean {
  if (includesQuery(group.project.name, normalizedQuery)) {
    return true;
  }

  return group.domains.some((domain) => {
    if (
      includesQuery(domain.registration.pattern, normalizedQuery) ||
      includesQuery(domain.registration.domain, normalizedQuery)
    ) {
      return true;
    }

    return domain.registration.routes.some(
      (route) =>
        includesQuery(route.path, normalizedQuery) ||
        includesQuery(route.target, normalizedQuery),
    );
  });
}

export function filterProjectGroups(groups: ProjectGroup[], query: string): ProjectGroup[] {
  const normalizedQuery = query.trim().toLocaleLowerCase();

  if (!normalizedQuery) {
    return groups;
  }

  return groups.filter((group) => matchesProjectGroup(group, normalizedQuery));
}
