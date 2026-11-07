import type { ReactNode } from "react";

// The centered h1 and lede at the top of the pricing and Localify Cloud pages.
export const PageIntro = ({
  eyebrow,
  title,
  description,
}: {
  eyebrow?: string;
  title: ReactNode;
  description: ReactNode;
}) => (
  <div className="mx-auto max-w-4xl space-y-6 text-center">
    {eyebrow ? (
      <span className="inline-flex items-center gap-2 font-mono text-[11px] tracking-[0.18em] text-muted-foreground uppercase">
        <span className="h-px w-6 bg-border" aria-hidden="true" />
        {eyebrow}
        <span className="h-px w-6 bg-border" aria-hidden="true" />
      </span>
    ) : null}
    <h1 className="text-heading-gradient text-[44px] leading-none font-semibold tracking-[-0.035em] text-balance md:text-[64px] lg:text-[72px]">
      {title}
    </h1>
    <p className="mx-auto max-w-xl text-[17px] leading-[1.55] text-pretty text-muted-foreground md:text-[18px]">
      {description}
    </p>
  </div>
);
