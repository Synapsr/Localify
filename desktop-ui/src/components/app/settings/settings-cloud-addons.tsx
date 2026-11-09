import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";

import type { CloudAddonDto, CloudAddonPricingDto, CloudPlanDto } from "@/types/localify";

import { ExternalLinkIcon, MinusIcon, PlusIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { cloudMutations, cloudQueries } from "@/lib/queries";
import {
  MAX_ADDON_UNITS,
  MIN_ADDON_UNITS,
  addonPriceLine,
  clampAddonUnits,
  hasAddons,
  heldAddonLine,
  heldAddonUnits,
} from "@/lib/util.cloud-addons";

const ADDONS: { addon: CloudAddonDto; label: string; unit: string }[] = [
  { addon: "machine", label: "+ Serving machines", unit: "serving machines" },
  { addon: "urls", label: "+ Public URLs (packs of 10)", unit: "packs of 10 URLs" },
];

/**
 * Settings → Localify Cloud, on a Pro plan: buys serving machines and public URLs on top of
 * it. Checkouts and the portal open in the browser through `DesktopApi`, and the plan card
 * shows the new limits once Localify Cloud sees them.
 */
export const SettingsCloudAddons = ({ plan }: { plan: CloudPlanDto }) => {
  const pricing = useQuery(cloudQueries.addonPricing());

  return (
    <div className="space-y-3 border-t border-border/40 pt-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-0.5">
          <div className="text-xs text-muted-foreground">Add capacity</div>
          <div className="text-xs leading-normal text-muted-foreground">
            Billed monthly, next to Pro.
          </div>
        </div>
        {hasAddons(plan) ? <ManageAddonsButton /> : null}
      </div>
      <div className="grid gap-4 sm:grid-cols-2">
        {ADDONS.map(({ addon, label, unit }) => (
          <AddonPurchase
            key={addon}
            addon={addon}
            label={label}
            unit={unit}
            held={heldAddonUnits(plan, addon)}
            pricing={pricing.data}
          />
        ))}
      </div>
    </div>
  );
};

/**
 * One add-on: a quantity and its price, then the checkout. Once the account holds it,
 * Localify Cloud answers with the portal, where its quantity is changed, so the row says so.
 */
const AddonPurchase = ({
  addon,
  label,
  unit,
  held,
  pricing,
}: {
  addon: CloudAddonDto;
  label: string;
  unit: string;
  /** Units the account already holds. */
  held: number;
  /** Undefined until Rust answers, with the live prices or the list ones. */
  pricing: CloudAddonPricingDto | undefined;
}) => {
  const queryClient = useQueryClient();
  const checkout = useMutation(cloudMutations.startAddonCheckout(queryClient));
  const [units, setUnits] = useState(MIN_ADDON_UNITS);
  const change = (next: number) => {
    checkout.reset();
    setUnits(clampAddonUnits(next));
  };

  const open = () =>
    checkout.mutate(
      { addon, units },
      {
        onSuccess: (page) =>
          toast.info(page.kind === "portal" ? "Customer portal opened" : "Checkout opened", {
            description:
              page.kind === "portal"
                ? "You already have this add-on: change its quantity in your browser."
                : "Finish it in your browser. The new limit shows here as soon as it counts.",
          }),
      },
    );

  const detail =
    held > 0 ? heldAddonLine(addon, held) : pricing ? addonPriceLine(addon, units, pricing) : null;

  return (
    <div className="min-w-0 space-y-2">
      <div className="text-sm text-foreground">{label}</div>
      <div className="flex flex-wrap items-center gap-2">
        {held > 0 ? null : (
          <div
            role="group"
            aria-label={`Number of ${unit}`}
            className="inline-flex items-center rounded-md bg-muted p-0.5 ring-1 ring-border/60"
          >
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              aria-label="One less"
              disabled={units <= MIN_ADDON_UNITS}
              onClick={() => change(units - 1)}
            >
              <MinusIcon />
            </Button>
            <span className="w-7 text-center text-xs font-medium text-foreground tabular-nums">
              {units}
            </span>
            <Button
              type="button"
              variant="ghost"
              size="icon-xs"
              aria-label="One more"
              disabled={units >= MAX_ADDON_UNITS}
              onClick={() => change(units + 1)}
            >
              <PlusIcon />
            </Button>
          </div>
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          loading={checkout.isPending}
          onClick={open}
        >
          {checkout.isPending ? null : <ExternalLinkIcon />}
          {held > 0 ? "Change in portal" : "Add"}
        </Button>
      </div>
      {detail ? <div className="text-xs leading-normal text-muted-foreground">{detail}</div> : null}
      {checkout.error ? (
        <div role="alert" className="text-xs leading-normal text-destructive">
          {checkout.error.message}
        </div>
      ) : null}
    </div>
  );
};

const ManageAddonsButton = () => {
  const queryClient = useQueryClient();
  const portal = useMutation(cloudMutations.openBillingPortal(queryClient));

  return (
    <div className="space-y-1 text-right">
      <Button
        type="button"
        variant="ghost"
        size="sm"
        loading={portal.isPending}
        onClick={() => portal.mutate()}
      >
        {portal.isPending ? null : <ExternalLinkIcon />}
        Manage add-ons
      </Button>
      {portal.error ? (
        <div role="alert" className="text-xs text-destructive">
          {portal.error.message}
        </div>
      ) : null}
    </div>
  );
};
