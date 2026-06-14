import { useCallback, useEffect, useMemo, useRef, useState } from "react";

const ORDER_STORAGE_KEY = "roxy.projectOrder";
const PINNED_STORAGE_KEY = "roxy.pinnedProjects";

export interface ProjectOrderActivity {
  id: string;
  hasReachableDomain: boolean;
  lastAccessedAt: number | null;
}

function loadStoredStringArray(key: string): string[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((value): value is string => typeof value === "string");
  } catch {
    return [];
  }
}

function saveStoredStringArray(key: string, values: string[]): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, JSON.stringify(values));
  } catch {
    // localStorage unavailable — ignore
  }
}

function reconcile(stored: string[], knownIds: string[]): string[] {
  const knownSet = new Set(knownIds);
  const kept = stored.filter((id) => knownSet.has(id));
  const keptSet = new Set(kept);
  const added = knownIds.filter((id) => !keptSet.has(id));
  return [...kept, ...added];
}

function keepKnown(stored: string[], knownIds: string[]): string[] {
  const knownSet = new Set(knownIds);
  return stored.filter((id) => knownSet.has(id));
}

function pinProjectsFirst(order: string[], pinnedIds: string[]): string[] {
  const pinnedSet = new Set(pinnedIds);
  return [
    ...order.filter((id) => pinnedSet.has(id)),
    ...order.filter((id) => !pinnedSet.has(id)),
  ];
}

function moveProjectToSectionTop(
  currentOrder: string[],
  projectId: string,
  pinnedIds: string[],
  knownIds: string[],
): string[] {
  if (!knownIds.includes(projectId)) {
    return reconcile(currentOrder, knownIds);
  }

  const pinnedSet = new Set(pinnedIds);
  const orderWithoutProject = reconcile(currentOrder, knownIds).filter(
    (id) => id !== projectId,
  );
  const insertIndex = pinnedSet.has(projectId)
    ? 0
    : orderWithoutProject.findIndex((id) => !pinnedSet.has(id));

  if (insertIndex === -1) {
    return [...orderWithoutProject, projectId];
  }

  return [
    ...orderWithoutProject.slice(0, insertIndex),
    projectId,
    ...orderWithoutProject.slice(insertIndex),
  ];
}

function latestActivity(
  activities: ProjectOrderActivity[],
): ProjectOrderActivity | null {
  return activities.reduce<ProjectOrderActivity | null>((latest, activity) => {
    if (activity.lastAccessedAt === null) {
      return latest;
    }

    const latestTimestamp = latest?.lastAccessedAt ?? null;
    if (latestTimestamp === null || activity.lastAccessedAt > latestTimestamp) {
      return activity;
    }

    return latest;
  }, null);
}

export function useProjectOrder(
  knownIds: string[],
  activities: ProjectOrderActivity[] = [],
) {
  const [order, setOrderState] = useState<string[]>(() =>
    reconcile(loadStoredStringArray(ORDER_STORAGE_KEY), knownIds),
  );
  const [pinnedIds, setPinnedIdsState] = useState<string[]>(() =>
    keepKnown(loadStoredStringArray(PINNED_STORAGE_KEY), knownIds),
  );
  const joinedKnown = knownIds.join("|");
  const previousKey = useRef(joinedKnown);
  const lastAppliedActivityKey = useRef<string | null>(null);
  const previousReachableIds = useRef<Set<string> | null>(null);

  useEffect(() => {
    if (previousKey.current === joinedKnown) return;
    previousKey.current = joinedKnown;
    setOrderState((current) => reconcile(current, knownIds));
    setPinnedIdsState((current) => keepKnown(current, knownIds));
  }, [joinedKnown, knownIds]);

  useEffect(() => {
    saveStoredStringArray(ORDER_STORAGE_KEY, order);
  }, [order]);

  useEffect(() => {
    saveStoredStringArray(PINNED_STORAGE_KEY, pinnedIds);
  }, [pinnedIds]);

  const mostRecentActivity = useMemo(() => latestActivity(activities), [activities]);
  const activityKey =
    mostRecentActivity?.lastAccessedAt === null || mostRecentActivity === null
      ? null
      : `${mostRecentActivity.id}:${mostRecentActivity.lastAccessedAt}`;
  const reachableIds = useMemo(
    () =>
      activities
        .filter((activity) => activity.hasReachableDomain)
        .map((activity) => activity.id),
    [activities],
  );
  const reachableKey = reachableIds.join("|");

  useEffect(() => {
    if (mostRecentActivity === null || activityKey === null) {
      return;
    }

    if (lastAppliedActivityKey.current === activityKey) {
      return;
    }

    lastAppliedActivityKey.current = activityKey;
    setOrderState((current) =>
      moveProjectToSectionTop(current, mostRecentActivity.id, pinnedIds, knownIds),
    );
  }, [activityKey, knownIds, mostRecentActivity, pinnedIds]);

  useEffect(() => {
    const currentReachableIds = new Set(reachableIds);
    const previous = previousReachableIds.current;
    previousReachableIds.current = currentReachableIds;

    const newlyReachableIds =
      previous === null
        ? reachableIds
        : reachableIds.filter((id) => !previous.has(id));

    if (newlyReachableIds.length === 0) {
      return;
    }

    setOrderState((current) => {
      const newlyReachableSet = new Set(newlyReachableIds);
      const orderedNewlyReachableIds = current.filter((id) =>
        newlyReachableSet.has(id),
      );

      return [...orderedNewlyReachableIds].reverse().reduce(
        (nextOrder, id) => moveProjectToSectionTop(nextOrder, id, pinnedIds, knownIds),
        current,
      );
    });
  }, [knownIds, pinnedIds, reachableIds, reachableKey]);

  const orderedIds = useMemo(() => pinProjectsFirst(order, pinnedIds), [order, pinnedIds]);

  const setOrder = useCallback(
    (nextOrder: string[]) => {
      setOrderState(reconcile(nextOrder, knownIds));
    },
    [knownIds],
  );

  const togglePinned = useCallback(
    (projectId: string) => {
      if (!knownIds.includes(projectId)) {
        return;
      }

      const isCurrentlyPinned = pinnedIds.includes(projectId);
      const nextPinnedIds = isCurrentlyPinned
        ? pinnedIds.filter((id) => id !== projectId)
        : keepKnown([...pinnedIds, projectId], knownIds);

      setPinnedIdsState(nextPinnedIds);
      setOrderState((current) =>
        isCurrentlyPinned
          ? reconcile(current, knownIds)
          : moveProjectToSectionTop(current, projectId, nextPinnedIds, knownIds),
      );
    },
    [knownIds, pinnedIds],
  );

  return { order: orderedIds, pinnedIds, setOrder, togglePinned };
}
