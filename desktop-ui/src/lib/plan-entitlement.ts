import type { CloudPlanDto } from "../types/localify";

/**
 * Not entitled to entitled, or back; or, while entitled stays the same, new limits, such as
 * an add-on bought or removed.
 */
export type EntitlementChange = "entitled" | "lost" | "limits" | null;

type PlanSnapshot = Pick<CloudPlanDto, "entitled" | "limits">;

/**
 * Tells when what the plan lets this account serve changes, however it changed: a checkout
 * here, the CLI, the portal, a payment that failed. The first answer, and a cloud that
 * reports no plan, set nothing to compare with.
 */
export function createEntitlementTracker() {
  let last: PlanSnapshot | undefined;
  return (plan: PlanSnapshot | null | undefined): EntitlementChange => {
    if (!plan) return null;
    const previous = last;
    last = { entitled: plan.entitled, limits: { ...plan.limits } };
    if (previous === undefined) return null;
    if (previous.entitled !== plan.entitled) return plan.entitled ? "entitled" : "lost";
    const limitsChanged =
      previous.limits.servingMachines !== plan.limits.servingMachines ||
      previous.limits.publicUrls !== plan.limits.publicUrls;
    return limitsChanged ? "limits" : null;
  };
}
