"use client";

import { useQuery } from "@tanstack/react-query";
import { Reorder } from "motion/react";
import { useMemo } from "react";

import { AppStartupScreen } from "@/components/shared/app-startup-screen";
import { bootstrapQueries } from "@/lib/queries";
import { buildProjectGroups } from "@/lib/util.project-groups";
import { useSharedProjectOrder } from "@/providers/project-order-provider";

import { ProjectsListEmptyState } from "./projects-list-empty-state";
import {
  ProjectListItemSkeleton,
  ProjectsListItem,
} from "./projects-list-item";
import { filterProjectGroups } from "./util.project-search";

interface ProjectsListProps {
  query: string;
}

export const ProjectsList = ({ query }: ProjectsListProps) => {
  const {
    data: bootstrap,
    error,
    isPending,
  } = useQuery(bootstrapQueries.read());

  const groups = useMemo(
    () =>
      buildProjectGroups(bootstrap?.projects ?? [], bootstrap?.domains ?? []),
    [bootstrap?.domains, bootstrap?.projects],
  );
  const groupById = useMemo(
    () => new Map(groups.map((group) => [group.project.id, group])),
    [groups],
  );
  const servicesByProjectId = useMemo(() => {
    const next = new Map<string, NonNullable<typeof bootstrap>["services"]>();
    for (const service of bootstrap?.services ?? []) {
      const services = next.get(service.projectId) ?? [];
      next.set(service.projectId, [...services, service]);
    }
    return next;
  }, [bootstrap?.services]);
  const environmentsByProjectId = useMemo(() => {
    const next = new Map<string, NonNullable<typeof bootstrap>["projectEnvironments"]>();
    for (const environment of bootstrap?.projectEnvironments ?? []) {
      const environments = next.get(environment.projectId) ?? [];
      next.set(environment.projectId, [...environments, environment]);
    }
    return next;
  }, [bootstrap?.projectEnvironments]);
  const { order, pinnedIds, setOrder, togglePinned } = useSharedProjectOrder();
  const pinnedProjectIds = useMemo(() => new Set(pinnedIds), [pinnedIds]);
  const filteredGroups = useMemo(
    () => filterProjectGroups(groups, query),
    [groups, query],
  );
  const filteredIds = useMemo(
    () => new Set(filteredGroups.map((group) => group.project.id)),
    [filteredGroups],
  );
  const orderedFilteredIds = useMemo(
    () => order.filter((id) => filteredIds.has(id)),
    [filteredIds, order],
  );
  const hasSearchQuery = query.trim().length > 0;

  if (isPending && !bootstrap) {
    return <ProjectListSkeleton />;
  }

  if (error && !bootstrap) {
    return (
      <AppStartupScreen
        eyebrow="Projects"
        title="Cannot load projects"
        message="Localify could not read your desktop projects."
        error={error.message}
      />
    );
  }

  if (groups.length === 0) {
    return <ProjectsListEmptyState query={query} />;
  }

  if (filteredGroups.length === 0) {
    return <ProjectsListEmptyState query={query} />;
  }

  if (hasSearchQuery) {
    return (
      <div className="flex flex-col gap-2">
        {orderedFilteredIds.map((id) => {
          const group = groupById.get(id);
          if (!group) return null;

          return (
            <ProjectsListItem
              key={id}
              project={group.project}
              domains={group.domains}
              services={servicesByProjectId.get(id) ?? []}
              environments={environmentsByProjectId.get(id) ?? []}
              isPinned={pinnedProjectIds.has(id)}
              onTogglePinned={() => togglePinned(id)}
              reorderEnabled={false}
            />
          );
        })}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 pb-(--bottom-padding-navigation)">
      <Reorder.Group
        axis="y"
        values={order}
        onReorder={setOrder}
        className="flex flex-col gap-2"
      >
        {order.map((id) => {
          const group = groupById.get(id);
          if (!group) return null;
          return (
            <ProjectsListItem
              key={id}
              project={group.project}
              domains={group.domains}
              services={servicesByProjectId.get(id) ?? []}
              environments={environmentsByProjectId.get(id) ?? []}
              isPinned={pinnedProjectIds.has(id)}
              onTogglePinned={() => togglePinned(id)}
            />
          );
        })}
      </Reorder.Group>
    </div>
  );
};

const ProjectListSkeleton = () => {
  return (
    <div className="flex flex-col gap-2">
      {Array.from({ length: 4 }).map((_, index) => (
        <ProjectListItemSkeleton key={index} />
      ))}
    </div>
  );
};
