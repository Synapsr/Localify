const PROJECT_CREATE_DIALOG_EVENT = "localify:projects-create-dialog-open";
const PROJECT_CREATE_DIALOG_STORAGE_KEY = "localify.projects.create-dialog-open";

function canUseWindow(): boolean {
  return typeof window !== "undefined";
}

export function requestProjectCreateDialogOpen(): void {
  if (!canUseWindow()) {
    return;
  }

  window.dispatchEvent(new Event(PROJECT_CREATE_DIALOG_EVENT));
}

export function queueProjectCreateDialogOpen(): void {
  if (!canUseWindow()) {
    return;
  }

  window.sessionStorage.setItem(PROJECT_CREATE_DIALOG_STORAGE_KEY, "1");
}

export function consumeQueuedProjectCreateDialogOpen(): boolean {
  if (!canUseWindow()) {
    return false;
  }

  const queued =
    window.sessionStorage.getItem(PROJECT_CREATE_DIALOG_STORAGE_KEY) === "1";

  if (queued) {
    window.sessionStorage.removeItem(PROJECT_CREATE_DIALOG_STORAGE_KEY);
  }

  return queued;
}

export function subscribeToProjectCreateDialogOpenRequest(
  onOpenRequested: () => void,
): () => void {
  if (!canUseWindow()) {
    return () => {};
  }

  window.addEventListener(PROJECT_CREATE_DIALOG_EVENT, onOpenRequested);

  return () => {
    window.removeEventListener(PROJECT_CREATE_DIALOG_EVENT, onOpenRequested);
  };
}
