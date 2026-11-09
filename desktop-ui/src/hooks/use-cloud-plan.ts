import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { toast } from "sonner";

import { cloudMutations, cloudQueries, entitlementChange } from "@/lib/queries";

/**
 * The account's plan, read only while signed in. When its entitlement or its limits change,
 * however they changed, the daemon is told to fetch what this Mac may serve, and the URLs
 * are read again: a machine add-on frees a serving slot for a blocked Mac.
 */
export function useCloudPlan(signedIn: boolean) {
  const queryClient = useQueryClient();
  const plan = useQuery({ ...cloudQueries.plan(), enabled: signedIn });
  const { mutate: refreshTunnel } = useMutation(cloudMutations.refreshTunnel(queryClient));

  useEffect(() => {
    const change = entitlementChange(plan.data);
    if (change === null) return;
    if (change === "entitled") {
      toast.success("Localify Cloud Pro is active", {
        description: "This Mac serves its public URLs.",
      });
    }
    if (change === "limits" && plan.data) {
      toast.success("Localify Cloud limits changed", {
        description: `${plan.data.limits.servingMachines} serving machines, ${plan.data.limits.publicUrls} public URLs.`,
      });
    }
    refreshTunnel(undefined, {
      onError: (error) =>
        toast.error("The daemon did not take the plan", { description: error.message }),
    });
  }, [plan.data, refreshTunnel]);

  return plan;
}
