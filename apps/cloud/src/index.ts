// Localify Cloud: the JSON API on api.<MACHINE_DOMAIN>, public URLs on
// *.<PUBLIC_BASE_DOMAIN>, and the scheduled jobs. One Worker, routed by host.

import { handleApi } from "./api";
import { purgeDeletedUrls, revokeStuckSignIns } from "./machines";
import { isPublicHost } from "./names";
import { handlePublic } from "./public";
import { runSweep } from "./tunnels";

/** The crons in wrangler.jsonc. */
const EVERY_FIVE_MINUTES = "*/5 * * * *";
const DAILY = "0 3 * * *";

export default {
  async fetch(request, env, ctx): Promise<Response> {
    const host = new URL(request.url).hostname;
    if (env.PUBLIC_BASE_DOMAIN && isPublicHost(host, env.PUBLIC_BASE_DOMAIN))
      return handlePublic(request, env);
    return handleApi(request, env, ctx);
  },

  async scheduled(controller, env, ctx): Promise<void> {
    if (controller.cron === EVERY_FIVE_MINUTES) ctx.waitUntil(everyFiveMinutes(env));
    else if (controller.cron === DAILY) ctx.waitUntil(purgeDeletedUrls(env));
    else console.error("Unknown cron", controller.cron);
  },
} satisfies ExportedHandler<Env>;

/** The tunnel sweep, which also retries the cleanup of revoked machines. */
async function everyFiveMinutes(env: Env): Promise<void> {
  const now = Date.now();
  await revokeStuckSignIns(env, now);
  await runSweep(env, now);
}
