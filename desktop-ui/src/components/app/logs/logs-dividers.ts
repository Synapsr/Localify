import { useCallback, useEffect, useState } from "react";

export interface Divider {
  id: string;
  at: number;
}

const STORAGE_KEY = "roxy.logs.dividers";

function loadDividers(): Divider[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (item): item is Divider =>
        typeof item === "object" &&
        item !== null &&
        typeof (item as Divider).id === "string" &&
        typeof (item as Divider).at === "number",
    );
  } catch {
    return [];
  }
}

function persist(dividers: Divider[]) {
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(dividers));
  } catch {
    // ignore
  }
}

function makeId(at: number): string {
  return `${at}-${Math.random().toString(36).slice(2, 8)}`;
}

export function useDividers() {
  const [dividers, setDividers] = useState<Divider[]>(() => loadDividers());

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== STORAGE_KEY) return;
      setDividers(loadDividers());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  const add = useCallback((at: number) => {
    setDividers((current) => {
      const next = [...current, { id: makeId(at), at }].sort((a, b) => a.at - b.at);
      persist(next);
      return next;
    });
  }, []);

  const remove = useCallback((id: string) => {
    setDividers((current) => {
      const next = current.filter((d) => d.id !== id);
      persist(next);
      return next;
    });
  }, []);

  const clear = useCallback(() => {
    setDividers(() => {
      persist([]);
      return [];
    });
  }, []);

  return { dividers, add, remove, clear };
}
