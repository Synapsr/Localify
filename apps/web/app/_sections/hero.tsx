import { RiGithubFill } from "@remixicon/react";
import * as motion from "motion/react-client";
import { getTranslations } from "next-intl/server";

import { AGENTS } from "@/app/_components/agent-logos";
import { AppWindow } from "@/app/_components/app-window";
import { DesktopFrame } from "@/app/_components/desktop-frame";
import { InstallPromptButton } from "@/app/_components/install-prompt-button";
import { UrlChipsBg } from "@/app/_components/url-chips-bg";
import { MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { ProjectsMockup } from "@/app/projects-mockup";
import { sourceUrl } from "@/app/site-metadata";
import { FadeIn, sectionClass } from "@/app/site-ui";
import { DownloadSolidIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";

const blurEase = [0.22, 1, 0.36, 1] as const;

const textContainer = {
  hidden: {},
  show: {
    transition: {
      staggerChildren: 0.12,
      delayChildren: 0.15,
    },
  },
};

const textItem = {
  hidden: { opacity: 0, filter: "blur(12px)", y: 10 },
  show: {
    opacity: 1,
    filter: "blur(0px)",
    y: 0,
    transition: { duration: 0.8, ease: blurEase },
  },
};

export const HeroSection = async () => {
  const t = await getTranslations("Hero");

  return (
    <section className={`${sectionClass} pt-16 md:pt-24`} id="top">
      <UrlChipsBg />

      <motion.div
        animate="show"
        className="relative mx-auto flex max-w-[870px] flex-col items-center space-y-7 text-center"
        initial="hidden"
        variants={textContainer}
      >
        <motion.span
          className="inline-flex items-center gap-2.5 rounded-full border border-border bg-background/80 py-1 pr-3.5 pl-1.5 text-[13px] font-medium text-foreground/80 shadow-[0_1px_2px_rgba(13,13,13,0.05)] backdrop-blur-sm"
          variants={textItem}
        >
          <span className="flex -space-x-1">
            {AGENTS.map(({ name, Logo, color }) => (
              <span
                className="inline-flex size-6 items-center justify-center rounded-full bg-background ring-1 ring-border"
                key={name}
              >
                <Logo aria-hidden="true" className="size-3.5" style={{ color }} />
              </span>
            ))}
          </span>
          {t("eyebrow")}
        </motion.span>

        <motion.h1
          className="text-heading-gradient text-[52px] leading-[0.98] font-semibold tracking-[-0.035em] text-balance sm:text-[72px] md:text-[96px] lg:text-[104px]"
          variants={textItem}
        >
          {t("titleLine1")}
          <br />
          {t("titleLine2")}
        </motion.h1>

        <motion.p
          className="max-w-2xl text-[19px] text-pretty text-muted-foreground md:text-[21px]"
          variants={textItem}
        >
          {t("description")}
        </motion.p>

        <motion.div
          className="flex flex-wrap items-center justify-center gap-3"
          variants={textItem}
        >
          <InstallPromptButton source="hero" />
          <Link
            className={buttonVariants({ variant: "outline", size: "lg" })}
            data-umami-event="download_clicked"
            data-umami-event-source="hero"
            href={MACOS_APP_DOWNLOAD_URL}
          >
            <DownloadSolidIcon className="h-4 w-4" />
            {t("download")}
          </Link>
        </motion.div>

        <motion.p
          className="max-w-md text-[13.5px] text-pretty text-muted-foreground"
          variants={textItem}
        >
          {t("promptHint")}{" "}
          <a
            className="inline-flex items-center gap-1 text-foreground underline decoration-border decoration-[1.5px] underline-offset-4 transition-colors hover:decoration-foreground"
            data-umami-event="source_clicked"
            data-umami-event-source="hero"
            href={sourceUrl}
            rel="noopener noreferrer"
            target="_blank"
          >
            <RiGithubFill className="size-3.5" />
            {t("source")}
          </a>
        </motion.p>
      </motion.div>

      <FadeIn delay={1.3} className="mt-16 md:mt-24">
        <DesktopFrame>
          <AppWindow>
            <ProjectsMockup />
          </AppWindow>
        </DesktopFrame>
      </FadeIn>

      <AgentStrip />
    </section>
  );
};

const AgentStrip = async () => {
  const t = await getTranslations("AgentStrip");

  return (
    <FadeIn className="mt-14 flex flex-col items-center gap-5 text-center md:mt-20">
      <p className="text-[13px] text-muted-foreground">{t("label")}</p>
      <div className="flex flex-wrap items-center justify-center gap-x-10 gap-y-4 text-foreground/75">
        {AGENTS.map(({ name, Logo, color }) => (
          <span className="inline-flex items-center gap-2.5 text-[20px] font-semibold tracking-tight md:text-[22px]" key={name}>
            <Logo aria-hidden="true" className="size-7" style={{ color }} />
            {name}
          </span>
        ))}
        <span className="text-[14px] text-muted-foreground">{t("more")}</span>
      </div>
    </FadeIn>
  );
};
