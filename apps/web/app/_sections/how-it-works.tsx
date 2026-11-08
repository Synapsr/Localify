import { getTranslations } from "next-intl/server";

import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";

type Node = { title: string; detail: string };

const DiagramNode = ({ node, emphasis = false }: { node: Node; emphasis?: boolean }) => (
  <div
    className={`flex h-full flex-col justify-center rounded-2xl px-4 py-3.5 text-center ring-1 ${
      emphasis ? "bg-foreground text-background ring-foreground" : "bg-muted/60 ring-border/60"
    }`}
  >
    <p className="text-[15px] font-semibold tracking-tight">{node.title}</p>
    <p className={`mt-0.5 font-mono text-[12px] ${emphasis ? "text-background/70" : "text-muted-foreground"}`}>
      {node.detail}
    </p>
  </div>
);

const Connector = () => (
  <div aria-hidden="true" className="mx-auto h-8 w-px bg-linear-to-b from-border to-foreground/40" />
);

export const HowItWorks = async () => {
  const t = await getTranslations("HowItWorks");
  const steps = t.raw("steps") as Array<{ title: string; body: string }>;
  const node = (key: string): Node => ({ title: t(`diagram.${key}`), detail: t(`diagram.${key}Detail`) });

  return (
    <section className={sectionClass} id="how-it-works">
      <FadeIn>
        <SectionHeading
          align="center"
          title={
            <>
              {t("titleLine1")}
              <br />
              {t("titleLine2")}
            </>
          }
        />
      </FadeIn>

      <div className="mt-16 grid gap-4 md:mt-20 md:grid-cols-3 md:gap-5">
        {steps.map((step, index) => (
          <FadeIn delay={0.05 + index * 0.08} key={step.title}>
            <article className="flex h-full flex-col gap-4 rounded-3xl bg-muted/60 p-7 ring-1 ring-border/60 md:p-8">
              <span className="inline-flex size-9 items-center justify-center rounded-full bg-foreground font-mono text-[13px] text-background">
                {index + 1}
              </span>
              <h3 className="text-[20px] leading-[1.2] font-medium tracking-tight text-balance text-foreground">
                {step.title}
              </h3>
              <p className="text-[14.5px] leading-[1.55] text-pretty text-muted-foreground">{step.body}</p>
            </article>
          </FadeIn>
        ))}
      </div>

      <FadeIn className="mx-auto mt-16 max-w-3xl md:mt-24" delay={0.1}>
        <div className="grid grid-cols-2 gap-3 md:gap-4">
          <DiagramNode node={node("you")} />
          <DiagramNode node={node("agents")} />
        </div>
        <Connector />
        <DiagramNode emphasis node={node("machine")} />
        <Connector />
        <div className="grid grid-cols-2 gap-3 md:gap-4">
          <DiagramNode node={node("servers")} />
          <div className="space-y-3 md:space-y-4">
            <DiagramNode node={node("cloud")} />
            <DiagramNode node={node("devices")} />
          </div>
        </div>
      </FadeIn>
    </section>
  );
};
