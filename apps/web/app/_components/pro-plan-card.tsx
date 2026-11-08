"use client";

import { useState } from "react";

import { CommandLine } from "./command-line";
import { PlanCard, PlanFeatures, PlanPrice } from "./plan-card";

type Interval = "monthly" | "yearly";

type IntervalOption = {
  label: string;
  price: string;
  period: string;
  note: string;
  command: string;
};

type ProPlanCardProps = {
  name: string;
  tagline: string;
  badge: string;
  intervalLabel: string;
  saveLabel: string;
  monthly: IntervalOption;
  yearly: IntervalOption;
  features: string[];
  trial: string;
  start: string;
};

export const ProPlanCard = ({
  name,
  tagline,
  badge,
  intervalLabel,
  saveLabel,
  monthly,
  yearly,
  features,
  trial,
  start,
}: ProPlanCardProps) => {
  const [interval, setBillingInterval] = useState<Interval>("monthly");
  const current = interval === "monthly" ? monthly : yearly;

  return (
    <PlanCard badge={badge} featured name={name} tagline={tagline}>
      <div className="space-y-5">
        <div
          aria-label={intervalLabel}
          className="inline-flex rounded-full bg-muted p-1 text-[13px] font-medium"
          role="group"
        >
          {(["monthly", "yearly"] as const).map((value) => {
            const option = value === "monthly" ? monthly : yearly;
            const active = interval === value;
            return (
              <button
                aria-pressed={active}
                className={`inline-flex items-center gap-1.5 rounded-full px-3.5 py-1.5 transition-colors duration-150 ${
                  active
                    ? "bg-background text-foreground shadow-soft ring-1 ring-border/60"
                    : "text-muted-foreground hover:text-foreground"
                }`}
                key={value}
                onClick={() => setBillingInterval(value)}
                type="button"
              >
                {option.label}
                {value === "yearly" ? (
                  <span className="text-[11.5px] font-normal text-muted-foreground">{saveLabel}</span>
                ) : null}
              </button>
            );
          })}
        </div>

        <PlanPrice note={current.note} period={current.period} price={current.price} />
      </div>

      <PlanFeatures items={features} />

      <div className="mt-auto space-y-3">
        <p className="text-[13.5px] leading-[1.55] text-pretty text-muted-foreground">{trial}</p>
        <p className="text-[14px] text-foreground">{start}</p>
        <CommandLine command={current.command} />
      </div>
    </PlanCard>
  );
};
