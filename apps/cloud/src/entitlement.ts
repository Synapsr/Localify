// What an account may use right now (docs/public-urls.md, "Plans").

import { isAllowedGithubId } from "./config";

export type Entitlement = { servingMachines: number; publicUrls: number };

const NO_ENTITLEMENT: Entitlement = { servingMachines: 0, publicUrls: 0 };
/** The limits the contract gives the overrides of the people on the old allowlist. */
const TEAM: Entitlement = { servingMachines: 5, publicUrls: 10 };

/**
 * The account's entitlement. Until plans and overrides are read, it is what
 * the allowlist gives: the team's limits on it, nothing off it.
 */
export async function entitlementOf(
  env: Env,
  user: { id: string; githubId: number },
): Promise<Entitlement> {
  return isAllowedGithubId(env, user.githubId) ? TEAM : NO_ENTITLEMENT;
}
