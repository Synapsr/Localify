"use client";

import { useQuery } from "@tanstack/react-query";
import { Reorder } from "motion/react";
import { useMemo } from "react";

import { AppStartupScreen } from "@/components/shared/app-startup-screen";
import { bootstrapQueries } from "@/lib/queries";
import { buildProjectGroups } from "@/lib/util.project-groups";
import { useProjectOrder } from "@/lib/use-project-order";

import { ProjectsListEmptyState } from "./projects-list-empty-state";
import {
  ProjectListItemSkeleton,
  ProjectsListItem,
} from "./projects-list-item";
import { ProjectsListUpsell } from "./projects-list-upsell";
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

  const licensing = bootstrap?.licensing;
  const atProjectLimit =
    licensing !== undefined &&
    licensing.status !== "licensed" &&
    licensing.status !== "licensedOffline" &&
    licensing.limits.maxProjects !== null &&
    licensing.projectCount >= licensing.limits.maxProjects;

  const groups = useMemo(
    () =>
      buildProjectGroups(bootstrap?.projects ?? [], bootstrap?.domains ?? []),
    [bootstrap?.domains, bootstrap?.projects],
  );
  const groupById = useMemo(
    () => new Map(groups.map((group) => [group.project.id, group])),
    [groups],
  );
  const knownIds = useMemo(
    () => groups.map((group) => group.project.id),
    [groups],
  );
  const projectActivities = useMemo(
    () =>
      groups.map((group) => ({
        id: group.project.id,
        hasReachableDomain: group.project.hasReachableDomain,
        lastAccessedAt: group.project.lastAccessedAt,
      })),
    [groups],
  );
  const { order, pinnedIds, setOrder, togglePinned } = useProjectOrder(
    knownIds,
    projectActivities,
  );
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
    <div className="flex flex-col gap-2">
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
              isPinned={pinnedProjectIds.has(id)}
              onTogglePinned={() => togglePinned(id)}
            />
          );
        })}
      </Reorder.Group>
      {atProjectLimit ? <ProjectsListUpsell /> : null}
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
