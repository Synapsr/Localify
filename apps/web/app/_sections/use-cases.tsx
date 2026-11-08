import { getTranslations } from "next-intl/server";
import type { ReactNode } from "react";

import { InstallPromptButton } from "@/app/_components/install-prompt-button";
import { AgentVisual, AppVisual, MachinesVisual, WorktreesVisual } from "@/app/_components/use-case-visuals";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";

const items = [
  { key: "agent", visual: <AgentVisual /> },
  { key: "worktrees", visual: <WorktreesVisual /> },
  { key: "app", visual: <AppVisual /> },
  { key: "machines", visual: <MachinesVisual /> },
] as const satisfies ReadonlyArray<{ key: string; visual: ReactNode }>;

export const UseCases = async () => {
  const t = await getTranslations("UseCases");

  return (
    <section className={sectionClass} id="use-cases">
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
          description={t("description")}
        />
      </FadeIn>

      <div className="mt-20 space-y-24 md:mt-28 md:space-y-36">
        {items.map((item, index) => (
          <div className="grid grid-cols-[minmax(0,1fr)] items-center gap-10 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] md:gap-16" key={item.key}>
            <FadeIn className={index % 2 === 1 ? "md:order-2" : ""}>
              <div className="space-y-5">
                <span className="inline-flex h-7 items-center rounded-full px-2.5 text-[13px] font-medium text-muted-foreground tabular-nums ring-1 ring-border">
                  {String(index + 1).padStart(2, "0")}
                </span>
                <h3 className="text-[32px] leading-[1.05] font-medium tracking-[-0.03em] text-balance text-foreground md:text-[40px]">
                  {t(`items.${item.key}.title`)}
                </h3>
                <p className="max-w-lg text-[16.5px] leading-[1.55] text-pretty text-muted-foreground">
                  {t(`items.${item.key}.body`)}
                </p>
                {item.key === "agent" ? (
                  <div className="flex flex-wrap items-center gap-3 pt-1">
                    <InstallPromptButton size="default" source="use_case_agent" variant="outline" />
                    <span className="text-[13px] text-muted-foreground">{t("items.agent.hint")}</span>
                  </div>
                ) : null}
              </div>
            </FadeIn>
            <FadeIn className={index % 2 === 1 ? "md:order-1" : ""} delay={0.1}>
              {item.visual}
            </FadeIn>
          </div>
        ))}
      </div>
    </section>
  );
};
