import {
  cleanText,
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

const EMAIL = /^[^\s@<>()[\]\\,;:"]+@[^\s@<>()[\]\\,;:"]+\.[^\s@<>()[\]\\,;:"]+$/;
const MAX_TEAM_SIZE = 100_000;

type ContactRequest = {
  name: string;
  email: string;
  company: string;
  size: number | null;
  message: string;
  interval: "monthly" | "yearly";
  price: string;
  locale: string;
  page: string;
};

// The Team contact form, checked field by field. Null when anything is off.
const parseContact = (body: Record<string, unknown>): ContactRequest | null => {
  const name = cleanText(body.name, 100);
  const email = cleanText(body.email, 254);
  const company = cleanText(body.company ?? "", 100);
  const message = cleanText(body.message ?? "", 2000);
  // The price the card showed, as the visitor read it, such as "€490 a year".
  const price = cleanText(body.price, 60);
  const rawSize = cleanText(String(body.size ?? ""), 10);
  const size = rawSize ? Number(rawSize) : null;

  if (
    body.plan !== "team" ||
    !name ||
    email === null ||
    !EMAIL.test(email) ||
    company === null ||
    message === null ||
    !price ||
    !isInterval(body.interval) ||
    rawSize === null ||
    (size !== null && (!Number.isInteger(size) || size < 1 || size > MAX_TEAM_SIZE)) ||
    !isLocale(body.locale) ||
    !isPage(body.page)
  ) {
    return null;
  }
  return { name, email, company, size, message, interval: body.interval, price, locale: body.locale, page: body.page };
};

const field = (name: string, value: string) => ({ name, value: value || "—", inline: true });

// A Team contact request from the pricing cards, posted to the team's Discord as an
// embed. The answer is only its status: 204 when Discord has it, 503 without the
// webhook, 502 when Discord fails, so the form can offer email instead.
export const POST = async (request: Request) => {
  const env = getEnv();
  const webhook = env.DISCORD_CONTACT_WEBHOOK_URL;
  if (!webhook) return emptyResponse(503);
  if (isCrossSite(request)) return emptyResponse(403);
  if (await isRateLimited(env.CONTACT_LIMITER, request)) return emptyResponse(429);

  const body = (await readJson(request, 8 * 1024)) as Record<string, unknown> | null | undefined;
  if (typeof body !== "object" || body === null) return emptyResponse(400);
  // The honeypot: people never see this field, so whatever fills it is a bot. It
  // gets the same answer as a person, and nothing is sent.
  if (body.website) return emptyResponse(204);

  const contact = parseContact(body);
  if (!contact) return emptyResponse(400);

  const sent = await postToDiscord(webhook, {
    embeds: [
      {
        title: "🛒 Localify Cloud Team request",
        description: contact.message || "—",
        color: 0x0d0d0d,
        fields: [
          field("Name", contact.name),
          field("Email", contact.email),
          field("Company", contact.company),
          field("Team size", contact.size === null ? "" : String(contact.size)),
          field("Billing", `${contact.interval} · ${contact.price}`),
          field("Locale", contact.locale),
          field("Page", contact.page),
        ],
        timestamp: new Date().toISOString(),
      },
    ],
  });
  return emptyResponse(sent ? 204 : 502);
};
