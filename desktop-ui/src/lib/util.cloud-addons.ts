import type { CloudAddonDto, CloudAddonPricingDto, CloudPlanDto } from "../types/localify";

/** A checkout sells 1 to 20 units of an add-on, the most Localify Cloud counts. */
export const MIN_ADDON_UNITS = 1;
export const MAX_ADDON_UNITS = 20;

/** Add-ons extend an entitled Pro plan; team access is not one. */
export function canAddCapacity(plan: CloudPlanDto): boolean {
  return plan.entitled && plan.status !== "none" && plan.status !== "override";
}

/** Whether the account holds an add-on, which the customer portal manages. */
export function hasAddons(plan: CloudPlanDto): boolean {
  return plan.addons.machines > 0 || plan.addons.urlPacks > 0;
}

/** The units of this add-on the account holds: once it holds some, buying opens the portal. */
export function heldAddonUnits(plan: CloudPlanDto, addon: CloudAddonDto): number {
  return addon === "machine" ? plan.addons.machines : plan.addons.urlPacks;
}

/** What the account holds of an add-on, and where to change it. */
export function heldAddonLine(addon: CloudAddonDto, units: number): string {
  const held =
    addon === "machine"
      ? `${units} extra serving ${units === 1 ? "machine" : "machines"}`
      : `${units * 10} extra public URLs`;
  return `You have ${held}. Change how many in the customer portal.`;
}

/** The stepper's value, kept within what a checkout sells. */
export function clampAddonUnits(units: number): number {
  if (!Number.isFinite(units)) return MIN_ADDON_UNITS;
  return Math.min(MAX_ADDON_UNITS, Math.max(MIN_ADDON_UNITS, Math.round(units)));
}

/** `$3`, `€2` or `$2.50`, from minor units. */
export function formatAddonPrice(amount: number, currency: CloudAddonPricingDto["currency"]) {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency,
    minimumFractionDigits: amount % 100 === 0 ? 0 : 2,
  }).format(amount / 100);
}

/**
 * The monthly price of `units` of an add-on, such as `$9 a month for 3 more serving
 * machines ($3 each)` or `$2 a month for 10 more public URLs`.
 */
export function addonPriceLine(
  addon: CloudAddonDto,
  units: number,
  pricing: CloudAddonPricingDto,
): string {
  const unitPrice = addon === "machine" ? pricing.machine : pricing.urlPack;
  const total = formatAddonPrice(unitPrice * units, pricing.currency);
  const each = formatAddonPrice(unitPrice, pricing.currency);
  if (addon === "machine") {
    return units === 1
      ? `${total} a month for 1 more serving machine`
      : `${total} a month for ${units} more serving machines (${each} each)`;
  }
  return units === 1
    ? `${total} a month for 10 more public URLs`
    : `${total} a month for ${units * 10} more public URLs (${each} per 10)`;
}
