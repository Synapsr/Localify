"use client";

import { useState } from "react";

import { AppleGlyph } from "@/app/_components/apple-glyph";
import { ProTrialLink } from "@/app/_components/pro-trial-link";
import { TeamContactButton } from "@/app/_components/team-contact";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { FadeIn } from "@/app/site-ui";
import { CheckIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";
import type { BillingInterval } from "@/lib/track";
import { cn } from "@/lib/utils";

// One plan's price for one interval, already formatted: "€79", "a year",
// "€6.58 a month, billed yearly · 2 months free".
export type IntervalPrice = { price: string; period: string; note: string };

type PaidPlan = {
  name: string;
  tagline: string;
  features: string[];
  cta: string;
  monthly: IntervalPrice;
  yearly: IntervalPrice;
};

export type PricingCardsProps = {
  source: "landing" | "pricing";
  intervals: { label: string; monthly: string; yearly: string; saving?: string };
  free: { name: string; price: string; period: string; tagline: string; features: string[]; cta: string };
  pro: PaidPlan & { badge: string };
  team: PaidPlan & { seats: number };
};

const Features = ({ items }: { items: string[] }) => (
  <ul className="space-y-3">
    {items.map((item) => (
      <li className="flex items-start gap-3 text-[14.5px] leading-[1.45] text-pretty" key={item}>
        <CheckIcon className="mt-0.5 size-4 shrink-0 opacity-70" strokeWidth={2.5} />
        {item}
      </li>
    ))}
  </ul>
);

// The note line is always there, empty on Free, so the features line up across cards.
const Price = ({ price, period, note, muted }: { price: string; period: string; note?: string; muted: string }) => (
  <div className="space-y-1.5">
    <p className="flex flex-wrap items-baseline gap-x-2">
      <span className="text-[44px] leading-none font-semibold tracking-[-0.03em] tabular-nums">{price}</span>
      <span className={cn("text-[14px]", muted)}>{period}</span>
    </p>
    <p className={cn("min-h-5 text-[13px]", muted)}>{note}</p>
  </div>
);

// The monthly/yearly switch and the Free, Pro and Team cards it prices.
export const PricingCards = ({ source, intervals, free, pro, team }: PricingCardsProps) => {
  const [interval, setBillingInterval] = useState<BillingInterval>("monthly");
  const card = "flex h-full flex-col gap-7 rounded-3xl p-7 md:p-8";
  const proPrice = pro[interval];
  const teamPrice = team[interval];

  return (
    <>
      <FadeIn className="mt-12 flex justify-center md:mt-16">
        <div
          aria-label={intervals.label}
          className="inline-flex rounded-full bg-muted/60 p-1 text-[14px] font-medium ring-1 ring-border/60"
          role="group"
        >
          {(["monthly", "yearly"] as const).map((value) => {
            const active = interval === value;
            return (
              <button
                aria-pressed={active}
                className={cn(
                  "inline-flex items-center gap-2 rounded-full px-4 py-2 transition-colors duration-150",
                  active
                    ? "bg-background text-foreground shadow-[0_1px_2px_rgba(13,13,13,0.08)] ring-1 ring-border/70"
                    : "text-muted-foreground hover:text-foreground",
                )}
                key={value}
                onClick={() => setBillingInterval(value)}
                type="button"
              >
                {intervals[value]}
                {value === "yearly" && intervals.saving ? (
                  <span className="rounded-full bg-foreground px-2 py-0.5 text-[11.5px] text-background">
                    {intervals.saving}
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
      </FadeIn>

      <div className="mx-auto mt-8 grid max-w-6xl gap-4 md:mt-10 lg:grid-cols-3 lg:gap-5">
        <FadeIn delay={0.05}>
          <article className={cn(card, "bg-muted/60 text-foreground ring-1 ring-border/60")}>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{free.name}</h3>
              <Price muted="text-muted-foreground" period={free.period} price={free.price} />
              <p className="text-[14.5px] text-muted-foreground">{free.tagline}</p>
            </header>
            <div className="flex-1">
              <Features items={free.features} />
            </div>
            <Link
              className={cn(buttonVariants({ variant: "outline", size: "lg" }), "w-full text-center whitespace-normal")}
              data-umami-event="download_clicked"
              data-umami-event-source={source}
              href={MACOS_APP_DOWNLOAD_URL}
            >
              <AppleGlyph className="h-4 w-4" />
              {free.cta}
            </Link>
          </article>
        </FadeIn>

        <FadeIn delay={0.12}>
          <article className={cn(card, "relative bg-foreground text-background shadow-[0_40px_80px_-40px_rgba(13,13,13,0.6)]")}>
            <span className="absolute top-7 right-7 rounded-full bg-background/15 px-2.5 py-1 text-[12px] font-medium md:top-8 md:right-8">
              {pro.badge}
            </span>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{pro.name}</h3>
              <Price muted="text-background/65" note={proPrice.note} period={proPrice.period} price={proPrice.price} />
              <p className="text-[14.5px] text-background/65">{pro.tagline}</p>
            </header>
            <div className="flex-1">
              <Features items={pro.features} />
            </div>
            <ProTrialLink
              className={cn(
                buttonVariants({ variant: "outline", size: "lg" }),
                "w-full border-background/20 bg-background text-center whitespace-normal text-foreground hover:bg-background/90",
              )}
              interval={interval}
              source={source}
            >
              {pro.cta}
            </ProTrialLink>
          </article>
        </FadeIn>

        <FadeIn delay={0.19}>
          <article className={cn(card, "bg-background text-foreground ring-1 ring-border")}>
            <header className="space-y-2">
              <h3 className="text-[15px] font-semibold">{team.name}</h3>
              <Price muted="text-muted-foreground" note={teamPrice.note} period={teamPrice.period} price={teamPrice.price} />
              <p className="text-[14.5px] text-muted-foreground">{team.tagline}</p>
            </header>
            <div className="flex-1">
              <Features items={team.features} />
            </div>
            <TeamContactButton
              className={cn(buttonVariants({ size: "lg" }), "w-full text-center whitespace-normal")}
              interval={interval}
              label={team.cta}
              planName={team.name}
              price={`${teamPrice.price} ${teamPrice.period}`}
              seats={team.seats}
              source={source}
            />
          </article>
        </FadeIn>
      </div>
    </>
  );
};
