// Localify Cloud: the JSON API on api.<MACHINE_DOMAIN>, public URLs on
// *.<PUBLIC_BASE_DOMAIN>, and an hourly cleanup. One Worker, routed by host.

import { handleApi } from "./api";
import { runCleanup } from "./machines";
import { isPublicHost } from "./names";
import { handlePublic } from "./public";

export default {
  async fetch(request, env): Promise<Response> {
    const host = new URL(request.url).hostname;
    if (env.PUBLIC_BASE_DOMAIN && isPublicHost(host, env.PUBLIC_BASE_DOMAIN))
      return handlePublic(request, env);
    return handleApi(request, env);
  },

  async scheduled(_controller, env, ctx): Promise<void> {
    ctx.waitUntil(runCleanup(env));
  },
} satisfies ExportedHandler<Env>;
