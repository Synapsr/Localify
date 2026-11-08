import { getTranslations } from "next-intl/server";
import type { ComponentType, ReactNode, SVGProps } from "react";

import { InstallPromptButton } from "@/app/_components/install-prompt-button";
import { AskDemo, ConnectDemo, PromptDemo } from "@/app/_components/step-demos";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";
import {
  ArrowRightIcon,
  CloudIcon,
  GitBranchIcon,
  GlobeIcon,
  LockIcon,
  MobileIcon,
  MonitorIcon,
  ServersIcon,
  SparklesIcon,
} from "@/components/icons";
import { Link } from "@/i18n/navigation";

type Icon = ComponentType<SVGProps<SVGSVGElement>>;

const demos = [<PromptDemo key="prompt" />, <ConnectDemo key="connect" />, <AskDemo key="ask" />];

export const HowItWorks = async () => {
  const t = await getTranslations("HowItWorks");
  const steps = t.raw("steps") as Array<{ title: string; body: string }>;

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

      <div className="mt-14 grid grid-cols-[minmax(0,1fr)] gap-5 md:mt-20 lg:grid-cols-3">
        {steps.map((step, index) => (
          <FadeIn className="flex flex-col gap-5" delay={0.05 + index * 0.08} key={step.title}>
            {demos[index]}
            <div className="space-y-2.5 px-1">
              <h3 className="flex items-center gap-2.5 text-[20px] leading-tight font-medium tracking-tight text-foreground">
                <span className="inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-foreground text-[12px] font-medium text-background tabular-nums">
                  {index + 1}
                </span>
                {step.title}
              </h3>
              <p className="text-[14.5px] leading-[1.55] text-pretty text-muted-foreground">{step.body}</p>
              {index === 0 ? (
                <div className="flex flex-wrap items-center gap-x-4 gap-y-2 pt-1">
                  <InstallPromptButton size="default" source="how_it_works" variant="outline" />
                  <Link
                    className="text-[13.5px] text-muted-foreground underline decoration-border decoration-[1.5px] underline-offset-4 hover:text-foreground hover:decoration-foreground"
                    data-umami-event="download_clicked"
                    data-umami-event-source="how_it_works"
                    href={MACOS_APP_DOWNLOAD_URL}
                  >
                    {t("dashboardLink")}
                  </Link>
                </div>
              ) : null}
            </div>
          </FadeIn>
        ))}
      </div>

      <Diagram
        labels={{
          title: t("diagram.title"),
          you: [t("diagram.you"), t("diagram.youDetail")],
          agents: [t("diagram.agents"), t("diagram.agentsDetail")],
          machine: t("diagram.machine"),
          dns: [t("diagram.dns"), t("diagram.dnsDetail")],
          https: [t("diagram.https"), t("diagram.httpsDetail")],
          router: [t("diagram.router"), t("diagram.routerDetail")],
          servers: [t("diagram.servers"), t("diagram.serversDetail")],
          cloud: [t("diagram.cloud"), t("diagram.cloudDetail")],
          devices: [t("diagram.devices"), t("diagram.devicesDetail")],
        }}
      />
    </section>
  );
};

type Pair = [string, string];

const Node = ({ icon: IconComponent, label, children }: { icon: Icon; label: Pair; children?: ReactNode }) => (
  <div className="rounded-2xl bg-muted/60 p-4 ring-1 ring-border/60">
    <div className="flex items-center gap-3">
      <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-background text-foreground/80 ring-1 ring-border/60">
        <IconComponent className="size-4" />
      </span>
      <div className="min-w-0">
        <p className="text-[14.5px] font-semibold tracking-tight text-foreground">{label[0]}</p>
        <p className="text-[12.5px] text-muted-foreground">{label[1]}</p>
      </div>
    </div>
    {children}
  </div>
);

const Arrow = () => (
  <div aria-hidden="true" className="flex items-center justify-center text-muted-foreground/70">
    <ArrowRightIcon className="size-5 rotate-90 md:rotate-0" />
  </div>
);

const Diagram = ({
  labels,
}: {
  labels: {
    title: string;
    you: Pair;
    agents: Pair;
    machine: string;
    dns: Pair;
    https: Pair;
    router: Pair;
    servers: Pair;
    cloud: Pair;
    devices: Pair;
  };
}) => (
  <FadeIn className="mt-24 md:mt-32">
    <h3 className="text-center text-[22px] font-medium tracking-tight text-foreground">{labels.title}</h3>
    <div className="mx-auto mt-10 grid max-w-5xl grid-cols-[minmax(0,1fr)] items-center gap-3 md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1.15fr)_auto_minmax(0,1fr)] md:gap-4">
      <div className="space-y-3">
        <Node icon={MonitorIcon} label={labels.you} />
        <Node icon={SparklesIcon} label={labels.agents} />
      </div>
      <Arrow />
      <div className="rounded-3xl bg-foreground p-5 text-background shadow-[0_30px_60px_-30px_rgba(13,13,13,0.5)]">
        <p className="text-[15px] font-semibold tracking-tight">{labels.machine}</p>
        <div className="mt-4 space-y-2.5">
          {(
            [
              [GlobeIcon, labels.dns],
              [LockIcon, labels.https],
              [GitBranchIcon, labels.router],
            ] as const
          ).map(([IconComponent, pair]) => (
            <div className="flex items-center gap-3 rounded-xl bg-background/10 px-3 py-2.5" key={pair[0]}>
              <IconComponent className="size-4 shrink-0 text-background/80" />
              <div className="min-w-0">
                <p className="text-[13.5px] font-medium">{pair[0]}</p>
                <p className="text-[12px] text-background/60">{pair[1]}</p>
              </div>
            </div>
          ))}
        </div>
      </div>
      <Arrow />
      <div className="space-y-3">
        <Node icon={ServersIcon} label={labels.servers}>
          <div className="mt-3 flex flex-wrap gap-1.5">
            {["web.journal", "web--fix-auth.journal", "api.journal"].map((host) => (
              <span className="inline-flex items-center gap-1.5 rounded-full border border-border bg-background px-2 py-1 text-[11.5px] text-foreground/80" key={host}>
                <span className="size-2 rounded-full bg-success" />
                {host}.localify
              </span>
            ))}
          </div>
        </Node>
        <Node icon={CloudIcon} label={labels.cloud}>
          <div className="mt-3 flex items-center gap-2 rounded-xl bg-background px-3 py-2 ring-1 ring-border/60">
            <MobileIcon className="size-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0">
              <p className="text-[12.5px] font-medium text-foreground">{labels.devices[0]}</p>
              <p className="truncate text-[11.5px] text-muted-foreground">{labels.devices[1]}</p>
            </div>
          </div>
        </Node>
      </div>
    </div>
  </FadeIn>
);
