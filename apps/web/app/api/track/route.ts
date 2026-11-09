import {
  emptyResponse,
  getEnv,
  isCrossSite,
  isInterval,
  isLocale,
  isPage,
  isRateLimited,
  postToDiscord,
  readJson,
} from "@/lib/discord";

const LABELS = { pro: "Pro trial", team: "Team" } as const;

// A click on a plan button (lib/track.ts), told to the team's Discord as one line,
// such as "🛒 Team · yearly clicked · fr · /pricing". It carries no identifier.
export const POST = async (request: Request) => {
  const env = getEnv();
  const webhook = env.DISCORD_CONTACT_WEBHOOK_URL;
  if (!webhook) return emptyResponse(503);
  if (isCrossSite(request)) return emptyResponse(403);
  if (await isRateLimited(env.TRACK_LIMITER, request)) return emptyResponse(429);

  const body = (await readJson(request, 1024)) as Record<string, unknown> | null | undefined;
  const plan = body?.plan;
  if (
    body?.event !== "plan_clicked" ||
    (plan !== "pro" && plan !== "team") ||
    !isInterval(body.interval) ||
    !isLocale(body.locale) ||
    !isPage(body.page)
  ) {
    return emptyResponse(400);
  }

  await postToDiscord(webhook, {
    content: `🛒 ${LABELS[plan]} · ${body.interval} clicked · ${body.locale} · ${body.page}`,
  });
  return emptyResponse(204);
};
