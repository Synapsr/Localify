import { useEffect, useRef, useState } from "react";

const STORAGE_KEY = "roxy.projectOrder";

function loadStoredOrder(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((value): value is string => typeof value === "string");
  } catch {
    return [];
  }
}

function saveStoredOrder(order: string[]): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(order));
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

export function useProjectOrder(knownIds: string[]) {
  const [order, setOrder] = useState<string[]>(() =>
    reconcile(loadStoredOrder(), knownIds),
  );
  const joinedKnown = knownIds.join("|");
  const previousKey = useRef(joinedKnown);

  useEffect(() => {
    if (previousKey.current === joinedKnown) return;
    previousKey.current = joinedKnown;
    setOrder((current) => reconcile(current, knownIds));
  }, [joinedKnown, knownIds]);

  useEffect(() => {
    saveStoredOrder(order);
  }, [order]);

  function reorder(from: number, to: number) {
    if (from === to) return;
    setOrder((current) => {
      if (
        from < 0 ||
        to < 0 ||
        from >= current.length ||
        to >= current.length
      ) {
        return current;
      }
      const next = [...current];
      const [moved] = next.splice(from, 1);
      next.splice(to, 0, moved);
      return next;
    });
  }

  return { order, reorder };
}
