import { RiGithubFill } from "@remixicon/react";
import * as motion from "motion/react-client";
import { getTranslations } from "next-intl/server";

import { AppWindow } from "@/app/_components/app-window";
import { DesktopFrame } from "@/app/_components/desktop-frame";
import { UrlChipsBg } from "@/app/_components/url-chips-bg";
import { CLI_INSTALL_COMMAND, MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { ProjectsMockup } from "@/app/projects-mockup";
import { sourceUrl } from "@/app/site-metadata";
import { FadeIn, sectionClass } from "@/app/site-ui";
import { DownloadSolidIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { Link } from "@/i18n/navigation";

const blurEase = [0.22, 1, 0.36, 1] as const;

const agentNames = ["Claude Code", "Codex", "Cursor"] as const;

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
          className="inline-flex items-center gap-2 font-mono text-[11.5px] tracking-[0.18em] text-muted-foreground uppercase"
          variants={textItem}
        >
          <span aria-hidden="true" className="size-1.5 rounded-full bg-emerald-500" />
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
          <Link
            className={buttonVariants({
              variant: "glow",
              size: "lg",
            })}
            data-umami-event="download_clicked"
            data-umami-event-source="hero"
            href={MACOS_APP_DOWNLOAD_URL}
          >
            <DownloadSolidIcon className="h-4 w-4" />
            {t("download")}
          </Link>
          <a
            className={buttonVariants({ variant: "outline", size: "lg" })}
            data-umami-event="source_clicked"
            data-umami-event-source="hero"
            href={sourceUrl}
            rel="noopener noreferrer"
            target="_blank"
          >
            <RiGithubFill className="h-4 w-4" />
            {t("source")}
          </a>
        </motion.div>

        <motion.div className="w-full max-w-md space-y-2" variants={textItem}>
          <p className="text-[13px] text-muted-foreground">{t("terminal")}</p>
          <code className="block overflow-x-auto rounded-lg border border-border/60 bg-background/80 px-3 py-2 font-mono text-[13px] whitespace-nowrap text-foreground select-all">
            {CLI_INSTALL_COMMAND}
          </code>
        </motion.div>

        <motion.p
          className="text-[13.5px] text-muted-foreground"
          variants={textItem}
        >
          {t("availability")}
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
    <FadeIn className="mt-14 flex flex-col items-center gap-4 text-center md:mt-20">
      <p className="text-[13px] text-muted-foreground">{t("label")}</p>
      <div className="flex flex-wrap items-center justify-center gap-x-10 gap-y-3 text-[20px] font-semibold tracking-tight text-foreground/70 md:text-[22px]">
        {agentNames.map((name) => (
          <span key={name}>{name}</span>
        ))}
        <span className="text-[14px] font-normal text-muted-foreground">{t("more")}</span>
      </div>
    </FadeIn>
  );
};
