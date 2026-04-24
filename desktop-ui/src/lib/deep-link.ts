import { listen } from "@tauri-apps/api/event";

import { router } from "@/router";
import type { DeepLinkPayloadDto } from "@/types/roxy";

import { tauriClient } from "./tauri-client";

const DEEP_LINK_EVENT = "localify://deep-link";

let bridgeStarted = false;

function normalizeView(view: string | null): "requests" | "raw" {
  return view === "raw" ? "raw" : "requests";
}

async function applyDeepLink(payload: DeepLinkPayloadDto): Promise<void> {
  if (payload.route === "projects") {
    await router.navigate({ to: "/projects" });
    return;
  }

  if (payload.route === "settingsLicensing") {
    await router.navigate({
      to: "/settings",
      search: {
        section: "licensing",
        action: "activateLicense",
      },
    });
    return;
  }

  await router.navigate({
    to: "/logs",
    search: {
      projectId: payload.projectId ?? undefined,
      view: normalizeView(payload.view),
    },
  });
}

export async function startDeepLinkBridge(): Promise<void> {
  if (bridgeStarted) {
    return;
  }
  bridgeStarted = true;

  const pending = await tauriClient.consumePendingDeepLink();
  if (pending) {
    await applyDeepLink(pending);
  }

  await listen<DeepLinkPayloadDto>(DEEP_LINK_EVENT, (event) => {
    void applyDeepLink(event.payload);
  });
}
