import { listen } from "@tauri-apps/api/event";

import type { DeepLinkLaunchDto, DeepLinkPayloadDto } from "@/types/localify";

import { router } from "@/router";

import { tauriClient } from "./tauri-client";

const DEEP_LINK_EVENT = "localify://deep-link";

let bridgeStarted = false;

type LaunchHandler = (launch: DeepLinkLaunchDto) => Promise<void>;

// The terminal panel starts services. A link that arrives before it is mounted,
// such as the one that opened the app, waits for it.
let launchHandler: LaunchHandler | null = null;
const waitingLaunches: DeepLinkLaunchDto[] = [];

export function setDeepLinkLaunchHandler(handler: LaunchHandler | null): void {
  launchHandler = handler;
  if (!handler) {
    return;
  }
  for (const launch of waitingLaunches.splice(0)) {
    void handler(launch);
  }
}

function normalizeView(view: string | null): "requests" | "raw" {
  return view === "raw" ? "raw" : "requests";
}

async function applyDeepLink(payload: DeepLinkPayloadDto): Promise<void> {
  if (payload.route === "launchService") {
    if (!payload.launch) {
      return;
    }
    if (launchHandler) {
      await launchHandler(payload.launch);
    } else {
      waitingLaunches.push(payload.launch);
    }
    return;
  }

  if (payload.route === "projects") {
    await router.navigate({ to: "/projects" });
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
