import type { CloudPlanDto } from "../types/localify";

/** Not entitled to entitled, or back. */
export type EntitlementChange = "entitled" | "lost" | null;

/**
 * Tells when the plan's entitlement changes, however it changed: a checkout here, the CLI,
 * the portal, a payment that failed. The first answer, and a cloud that reports no plan,
 * set nothing to compare with.
 */
export function createEntitlementTracker() {
  let last: boolean | undefined;
  return (plan: Pick<CloudPlanDto, "entitled"> | null | undefined): EntitlementChange => {
    if (!plan) return null;
    const previous = last;
    last = plan.entitled;
    if (previous === undefined || previous === plan.entitled) return null;
    return plan.entitled ? "entitled" : "lost";
  };
}
