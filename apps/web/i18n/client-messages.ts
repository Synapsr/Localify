import type { AbstractIntlMessages } from "next-intl";

// The namespaces client components read. Sending only these keeps the legal pages'
// text out of the payload of every other page.
const clientNamespaces = [
  "Agents",
  "CookieBanner",
  "DemoDialog",
  "Faq",
  "Footer",
  "HowItWorks",
] as const;

export const pickClientMessages = (messages: AbstractIntlMessages): AbstractIntlMessages =>
  Object.fromEntries(clientNamespaces.map((namespace) => [namespace, messages[namespace]]));
