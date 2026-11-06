import { RiGithubFill } from "@remixicon/react";
import * as motion from "motion/react-client";
import { getTranslations } from "next-intl/server";
import Image from "next/image";
import { Link } from "@/i18n/navigation";

import { AppleGlyph } from "@/app/_components/apple-glyph";
import { CLI_INSTALL_COMMAND, MACOS_APP_DOWNLOAD_URL } from "@/app/download-links";
import { sourceUrl } from "@/app/site-metadata";
import { sectionClass } from "@/app/site-ui";
import { buttonVariants } from "@/components/ui/button-variants";

const blurEase = [0.22, 1, 0.36, 1] as const;

const container = {
  hidden: {},
  show: {
    transition: { staggerChildren: 0.12, delayChildren: 0.1 },
  },
};

const item = {
  hidden: { opacity: 0, filter: "blur(12px)", y: 10 },
  show: {
    opacity: 1,
    filter: "blur(0px)",
    y: 0,
    transition: { duration: 0.8, ease: blurEase },
  },
};

export const FinalCta = async () => {
  const t = await getTranslations("FinalCta");

  return (
    <section className={sectionClass} id="download">
      <motion.div
        animate="show"
        className="relative mx-auto flex max-w-2xl flex-col items-center space-y-6 text-center"
        initial="hidden"
        variants={container}
        viewport={{ once: true, amount: 0.4 }}
        whileInView="show"
      >
        <motion.div variants={item}>
          <Image
            alt="Localify"
            className="h-[112px] w-[112px] select-none"
            draggable={false}
            height={224}
            priority={false}
            src="/logo-light.png"
            width={224}
          />
        </motion.div>

        <motion.h2 className="flex items-center" variants={item}>
          <Image
            alt="Localify"
            className="h-10 w-auto select-none md:h-12"
            draggable={false}
            height={66}
            src="/localify-text.svg"
            width={246}
          />
        </motion.h2>

        <motion.p
          className="max-w-md text-[17px] leading-normal text-pretty text-muted-foreground md:text-[18px]"
          variants={item}
        >
          {t("description")}
        </motion.p>

        <motion.div
          className="flex flex-wrap items-center justify-center gap-3 pt-2"
          variants={item}
        >
          <Link
            className={buttonVariants({ variant: "glow", size: "lg" })}
            data-umami-event="download_clicked"
            data-umami-event-source="final_cta"
            href={MACOS_APP_DOWNLOAD_URL}
          >
            <AppleGlyph className="h-4 w-4" />
            {t("download")}
          </Link>
          <a
            className={buttonVariants({ variant: "outline", size: "lg" })}
            data-umami-event="source_clicked"
            data-umami-event-source="final_cta"
            href={sourceUrl}
            rel="noopener noreferrer"
            target="_blank"
          >
            <RiGithubFill className="h-4 w-4" />
            {t("source")}
          </a>
        </motion.div>

        <motion.div className="w-full max-w-md space-y-2" variants={item}>
          <p className="text-[13px] text-muted-foreground">{t("terminal")}</p>
          <code className="block overflow-x-auto rounded-lg border border-border/60 bg-background/80 px-3 py-2 font-mono text-[13px] whitespace-nowrap text-foreground select-all">
            {CLI_INSTALL_COMMAND}
          </code>
        </motion.div>

        <motion.p className="text-[13px] text-muted-foreground" variants={item}>
          {t("requirements")}
        </motion.p>
      </motion.div>
    </section>
  );
};
