import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";

import type { CloudPlanDto, CloudPlanIntervalDto } from "@/types/localify";

import { ExternalLinkIcon, SparklesIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { cloudMutations } from "@/lib/queries";
import { PRO_PRICE } from "@/lib/util.cloud";
import { cn } from "@/lib/utils";

const INTERVALS: CloudPlanIntervalDto[] = ["month", "year"];

/**
 * Without a plan: the monthly or yearly choice and the trial button, or Subscribe once the
 * trial is used. The checkout opens in the browser through `DesktopApi`.
 */
export const CloudSubscribeAction = ({ plan }: { plan: CloudPlanDto | null | undefined }) => {
  const queryClient = useQueryClient();
  const checkout = useMutation(cloudMutations.startCheckout(queryClient));
  const [interval, setChosenInterval] = useState<CloudPlanIntervalDto>("month");
  const trial = plan?.trialAvailable ?? true;

  const start = () =>
    checkout.mutate(
      { interval },
      {
        onSuccess: (page) =>
          toast.info(page.kind === "portal" ? "The plan is already active" : "Checkout opened", {
            description:
              page.kind === "portal"
                ? "The customer portal opened in your browser, where the interval can be switched."
                : "Finish it in your browser. Localify sees the plan as soon as it starts.",
          }),
        onError: (error) =>
          toast.error("Could not open the checkout", { description: error.message }),
      },
    );

  return (
    <div className="flex flex-wrap items-center gap-2">
      <div
        role="radiogroup"
        aria-label="Billing interval"
        className="inline-flex rounded-md bg-muted p-0.5 ring-1 ring-border/60"
      >
        {INTERVALS.map((value) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={interval === value}
            onClick={() => setChosenInterval(value)}
            className={cn(
              "rounded-[5px] px-2.5 py-1 text-xs font-medium text-muted-foreground transition-colors",
              interval === value && "bg-background text-foreground shadow-xs",
            )}
          >
            {PRO_PRICE[value]}
          </button>
        ))}
      </div>
      <Button type="button" variant="primary" loading={checkout.isPending} onClick={start}>
        {checkout.isPending ? null : trial ? <SparklesIcon /> : <ExternalLinkIcon />}
        {trial ? "Start 7-day free trial" : "Subscribe"}
      </Button>
    </div>
  );
};
