import type { ReactNode } from "react";

import { CheckIcon } from "@/components/icons";

type PlanCardProps = {
  name: string;
  tagline: string;
  badge?: string;
  featured?: boolean;
  children: ReactNode;
};

export const PlanCard = ({ name, tagline, badge, featured = false, children }: PlanCardProps) => {
  const surface = featured
    ? "bg-background shadow-lift ring-foreground/15"
    : "bg-muted/60 ring-border/60";

  return (
    <article
      className={`flex h-full min-w-0 flex-col gap-7 rounded-3xl p-7 ring-1 md:p-8 ${surface}`}
    >
      <header className="space-y-2">
        <div className="flex items-center justify-between gap-3">
          <h2 className="text-ink text-[22px] font-semibold tracking-tight">{name}</h2>
          {badge ? (
            <span className="rounded-full bg-background px-2.5 py-1 font-mono text-[10.5px] tracking-[0.12em] text-muted-foreground uppercase ring-1 ring-border/70">
              {badge}
            </span>
          ) : null}
        </div>
        <p className="text-[15px] leading-[1.5] text-pretty text-muted-foreground">{tagline}</p>
      </header>
      {children}
    </article>
  );
};

export const PlanPrice = ({ price, period, note }: { price: string; period?: string; note: string }) => (
  <div className="space-y-1.5">
    <p className="flex items-baseline gap-1.5">
      <span className="text-ink text-[44px] leading-none font-semibold tracking-[-0.035em] tabular-nums">
        {price}
      </span>
      {period ? <span className="text-[15px] text-muted-foreground">{period}</span> : null}
    </p>
    <p className="text-[13.5px] text-muted-foreground">{note}</p>
  </div>
);

export const PlanFeatures = ({ items }: { items: string[] }) => (
  <ul className="space-y-2.5 text-[14.5px] leading-[1.5] text-foreground">
    {items.map((item) => (
      <li key={item} className="flex gap-2.5">
        <CheckIcon aria-hidden="true" className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" strokeWidth={2} />
        <span className="text-pretty">{item}</span>
      </li>
    ))}
  </ul>
);
