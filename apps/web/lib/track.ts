export type BillingInterval = "monthly" | "yearly";

export type PlanClick = { plan: "pro" | "team"; interval: BillingInterval; locale: string; page: string };

// Tells the team, through /api/track, that a plan button was clicked, with the billing
// interval the switch showed. Fire and forget: it never delays the click, and a failure
// is silent. sendBeacon survives the navigation the Pro button starts; fetch with
// keepalive covers browsers without it.
export const trackPlanClick = ({ plan, interval, locale, page }: PlanClick) => {
  const body = JSON.stringify({ event: "plan_clicked", plan, interval, locale, page });
  try {
    if (navigator.sendBeacon?.("/api/track", body)) return;
  } catch {
    // Fall through to fetch.
  }
  void fetch("/api/track", { method: "POST", body, keepalive: true }).catch(() => undefined);
};
