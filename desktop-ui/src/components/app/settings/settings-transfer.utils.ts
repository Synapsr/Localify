type TransferProjectLike = {
  id: string;
};

const countSelectedNewProjects = (
  selectedProjectIds: string[],
  currentProjectIds: string[],
): number => {
  const currentProjectIdSet = new Set(currentProjectIds);

  return selectedProjectIds.filter((projectId) => !currentProjectIdSet.has(projectId)).length;
};

export const getAvailableImportProjectSlots = (
  maxProjects: number | null,
  currentProjectCount: number,
): number | null => {
  if (maxProjects === null) {
    return null;
  }

  return Math.max(maxProjects - currentProjectCount, 0);
};

export const buildInitialImportProjectSelection = <TProject extends TransferProjectLike>({
  currentProjectIds,
  maxProjects,
  projects,
}: {
  currentProjectIds: string[];
  maxProjects: number | null;
  projects: TProject[];
}): string[] => {
  if (maxProjects === null) {
    return projects.map((project) => project.id);
  }

  const currentProjectIdSet = new Set(currentProjectIds);
  let remainingSlots = getAvailableImportProjectSlots(maxProjects, currentProjectIds.length) ?? 0;

  return projects.flatMap((project) => {
    if (currentProjectIdSet.has(project.id)) {
      return [project.id];
    }

    if (remainingSlots === 0) {
      return [];
    }

    remainingSlots -= 1;
    return [project.id];
  });
};

export const canSelectImportProject = ({
  currentProjectIds,
  maxProjects,
  projectId,
  selectedProjectIds,
}: {
  currentProjectIds: string[];
  maxProjects: number | null;
  projectId: string;
  selectedProjectIds: string[];
}): boolean => {
  if (selectedProjectIds.includes(projectId) || maxProjects === null) {
    return true;
  }

  const currentProjectIdSet = new Set(currentProjectIds);

  if (currentProjectIdSet.has(projectId)) {
    return true;
  }

  const remainingSlots =
    (getAvailableImportProjectSlots(maxProjects, currentProjectIds.length) ?? 0) -
    countSelectedNewProjects(selectedProjectIds, currentProjectIds);

  return remainingSlots > 0;
};

export const countSelectedImportProjectsUsingNewSlots = (
  selectedProjectIds: string[],
  currentProjectIds: string[],
): number => countSelectedNewProjects(selectedProjectIds, currentProjectIds);
