const PROJECT_SEARCH_FOCUS_EVENT = "localify:projects-search-focus";
const PROJECT_SEARCH_FOCUS_STORAGE_KEY = "localify.projects.focus-search";

function canUseWindow(): boolean {
  return typeof window !== "undefined";
}

export function requestProjectsSearchFocus(): void {
  if (!canUseWindow()) {
    return;
  }

  window.dispatchEvent(new Event(PROJECT_SEARCH_FOCUS_EVENT));
}

export function queueProjectsSearchFocus(): void {
  if (!canUseWindow()) {
    return;
  }

  window.sessionStorage.setItem(PROJECT_SEARCH_FOCUS_STORAGE_KEY, "1");
}

export function consumeQueuedProjectsSearchFocus(): boolean {
  if (!canUseWindow()) {
    return false;
  }

  const queued = window.sessionStorage.getItem(PROJECT_SEARCH_FOCUS_STORAGE_KEY) === "1";
  if (queued) {
    window.sessionStorage.removeItem(PROJECT_SEARCH_FOCUS_STORAGE_KEY);
  }

  return queued;
}

export function subscribeToProjectsSearchFocusRequest(
  onFocusRequested: () => void,
): () => void {
  if (!canUseWindow()) {
    return () => {};
  }

  const handleFocusRequest = () => {
    onFocusRequested();
  };

  window.addEventListener(PROJECT_SEARCH_FOCUS_EVENT, handleFocusRequest);

  return () => {
    window.removeEventListener(PROJECT_SEARCH_FOCUS_EVENT, handleFocusRequest);
  };
}
