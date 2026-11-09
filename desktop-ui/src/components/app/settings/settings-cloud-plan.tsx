import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import type { CloudPlanDto } from "@/types/localify";

import { CloudSubscribeAction } from "@/components/app/cloud/cloud-subscribe-action";
import { ExternalLinkIcon, SpinnerIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { useCloudPlan } from "@/hooks/use-cloud-plan";
import { cloudMutations } from "@/lib/queries";
import { describePlan } from "@/lib/util.cloud";
import { canAddCapacity } from "@/lib/util.cloud-addons";

import { SettingsCloudAddons } from "./settings-cloud-addons";

/** Settings → Localify Cloud: the plan, its usage, one action, and add-ons on Pro. */
export const SettingsCloudPlan = () => {
  const plan = useCloudPlan(true);

  if (plan.isPending) {
    return (
      <section className="mt-5 flex items-center gap-2 border-t border-border/40 pt-5 text-xs text-muted-foreground">
        <SpinnerIcon className="size-3.5 animate-spin" />
        Checking the plan…
      </section>
    );
  }
  if (plan.error) {
    return (
      <section className="mt-5 border-t border-border/40 pt-5 text-xs text-muted-foreground">
        The plan could not be read: {plan.error.message}
      </section>
    );
  }
  // A Localify Cloud that does not report plans yet.
  if (!plan.data) return null;

  return <PlanDetails plan={plan.data} />;
};

const PlanDetails = ({ plan }: { plan: CloudPlanDto }) => {
  const { label, detail } = describePlan(plan);
  const hasPlan = plan.status !== "none";

  return (
    <section className="mt-5 space-y-4 border-t border-border/40 pt-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-0.5">
          <div className="text-xs text-muted-foreground">Plan</div>
          <div className="text-sm text-foreground">{label}</div>
          {detail ? (
            <div className="text-xs leading-normal text-muted-foreground">{detail}</div>
          ) : null}
        </div>
        {hasPlan ? <ManageBillingButton /> : <CloudSubscribeAction plan={plan} />}
      </div>
      {hasPlan ? (
        <dl className="grid gap-4 sm:grid-cols-2">
          <Usage
            label="Serving machines"
            used={plan.usage.servingMachines}
            limit={plan.limits.servingMachines}
          />
          <Usage label="Public URLs" used={plan.usage.publicUrls} limit={plan.limits.publicUrls} />
        </dl>
      ) : null}
      {canAddCapacity(plan) ? <SettingsCloudAddons plan={plan} /> : null}
    </section>
  );
};

const Usage = ({ label, used, limit }: { label: string; used: number; limit: number }) => (
  <div className="min-w-0 space-y-1">
    <dt className="text-xs text-muted-foreground">{label}</dt>
    <dd className="text-sm text-foreground tabular-nums">
      {used} of {limit}
    </dd>
    <dd aria-hidden className="h-1 overflow-hidden rounded-full bg-muted">
      <div
        className="h-full rounded-full bg-primary"
        style={{ width: `${limit > 0 ? Math.min(100, (used / limit) * 100) : 0}%` }}
      />
    </dd>
  </div>
);

const ManageBillingButton = () => {
  const queryClient = useQueryClient();
  const portal = useMutation(cloudMutations.openBillingPortal(queryClient));

  return (
    <Button
      type="button"
      variant="outline"
      loading={portal.isPending}
      onClick={() =>
        portal.mutate(undefined, {
          onError: (error) => toast.error("Could not open billing", { description: error.message }),
        })
      }
    >
      {portal.isPending ? null : <ExternalLinkIcon />}
      Manage billing
    </Button>
  );
};
