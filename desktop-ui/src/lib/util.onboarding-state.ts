const ONBOARDING_COMPLETED_STORAGE_KEY = "localify.onboarding-completed";

export function loadStoredOnboardingCompleted(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  return window.localStorage.getItem(ONBOARDING_COMPLETED_STORAGE_KEY) === "1";
}

export function markStoredOnboardingCompleted(): void {
  if (typeof window === "undefined") {
    return;
  }

  window.localStorage.setItem(ONBOARDING_COMPLETED_STORAGE_KEY, "1");
}

export function clearStoredOnboardingCompleted(): void {
  if (typeof window === "undefined") {
    return;
  }

  window.localStorage.removeItem(ONBOARDING_COMPLETED_STORAGE_KEY);
}
