// What raises a limit the API refuses at (docs/public-urls.md, "API"): the
// `upgrade` object of `402 subscription_required`, `403 quota_exceeded` and
// a blocked runtime.
//
// It names a command, never a checkout or portal link: making one costs a
// Polar call and a pending checkout on a refusal path, and an agent that
// reads the error must never pass a checkout link on (ADR 0018).

import type { Account } from "./entitlement";

import { addonsOnSale } from "./config";

export type Upgrade = {
  kind: "pro" | "addon_machine" | "addon_urls";
  command: "localify subscribe" | "localify billing";
};

/** For an account with no entitlement: Pro, and its trial. */
export const PRO_UPGRADE: Upgrade = { kind: "pro", command: "localify subscribe" };

/**
 * For a limit an add-on raises, or null when none can: add-ons count only on
 * an entitled Pro plan, never under an override, and are sold only while
 * `ADDONS` is `on`.
 */
export function addonUpgrade(
  env: Env,
  account: Account | null,
  kind: "addon_machine" | "addon_urls",
): Upgrade | null {
  if (!account || !account.plan.entitled || account.plan.status === "override") return null;
  return addonsOnSale(env) ? { kind, command: "localify billing" } : null;
}
