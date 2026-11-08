"use client";

import { AnimatePresence, motion } from "motion/react";
import { useTranslations } from "next-intl";
import { useState } from "react";

import { FadeIn, sectionClass } from "@/app/site-ui";
import { ChevronDownIcon } from "@/components/icons";
import { Link } from "@/i18n/navigation";

export type FaqEntry = {
  question: string;
  answer: string;
  link?: { label: string; href: string };
};

const ease = [0.22, 1, 0.36, 1] as const;
const blurEase = [0.22, 1, 0.36, 1] as const;

const headingContainer = {
  hidden: {},
  show: {
    transition: { staggerChildren: 0.12, delayChildren: 0.1 },
  },
};

const headingItem = {
  hidden: { opacity: 0, filter: "blur(12px)", y: 10 },
  show: {
    opacity: 1,
    filter: "blur(0px)",
    y: 0,
    transition: { duration: 0.8, ease: blurEase },
  },
};

export const Faq = () => {
  const t = useTranslations("Faq");
  const items = t.raw("items") as FaqEntry[];

  return (
    <section className={sectionClass} id="faq">
      <motion.div
        animate="show"
        className="mx-auto flex max-w-3xl flex-col items-center space-y-6 text-center"
        initial="hidden"
        variants={headingContainer}
        viewport={{ once: true, amount: 0.4 }}
        whileInView="show"
      >
        <motion.h2
          className="text-heading-gradient text-[44px] leading-none font-semibold tracking-[-0.035em] text-balance md:text-[64px] lg:text-[72px]"
          variants={headingItem}
        >
          {t("titleLine1")}
          <br />
          {t("titleLine2")}
        </motion.h2>

        <motion.p
          className="max-w-xl text-[17px] leading-[1.55] text-pretty text-muted-foreground md:text-[18px]"
          variants={headingItem}
        >
          {t("description")}
        </motion.p>
      </motion.div>

      <FadeIn className="mx-auto mt-16 max-w-3xl md:mt-20">
        <div className="flex flex-col gap-2.5">
          {items.map((item) => (
            <FaqItem key={item.question} {...item} />
          ))}
        </div>

        <p className="mt-12 text-center text-[14px] text-muted-foreground">
          {t("supportPrefix")}{" "}
          <a
            className="text-foreground underline decoration-border decoration-[1.5px] underline-offset-[5px] transition-colors hover:decoration-foreground"
            href="mailto:support@localify.dev"
          >
            support@localify.dev
          </a>
        </p>
      </FadeIn>
    </section>
  );
};

export const FaqItem = ({ answer, question, link }: FaqEntry) => {
  const [open, setOpen] = useState(false);
  return (
    <div
      className="rounded-[22px] bg-muted/60 ring-1 ring-border/60 transition-colors duration-200 hover:bg-muted"
      data-open={open}
    >
      <button
        aria-expanded={open}
        className="group flex w-full items-center justify-between gap-6 px-6 py-5 text-left md:px-7 md:py-6"
        onClick={() => setOpen((v) => !v)}
        type="button"
      >
        <span className="text-lg font-semibold tracking-tight text-balance text-foreground md:text-[17.5px]">
          {question}
        </span>
        <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-full ring-1 ring-border/60 transition-colors duration-200">
          <ChevronDownIcon
            className={`h-4 w-4 text-muted-foreground transition-[transform,color] duration-300 group-hover:text-foreground ${
              open ? "rotate-180" : ""
            }`}
            strokeWidth={2}
          />
        </span>
      </button>
      <AnimatePresence initial={false}>
        {open ? (
          <motion.div
            animate={{ height: "auto", opacity: 1 }}
            className="overflow-hidden"
            exit={{ height: 0, opacity: 0 }}
            initial={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.3, ease }}
          >
            <p className="max-w-2xl px-6 pb-6 text-[15px] leading-[1.65] text-pretty text-muted-foreground md:px-7">
              {answer}
              {link ? (
                <>
                  {" "}
                  <Link
                    className="text-foreground underline decoration-border decoration-[1.5px] underline-offset-[5px] transition-colors hover:decoration-foreground"
                    href={link.href}
                  >
                    {link.label}
                  </Link>
                </>
              ) : null}
            </p>
          </motion.div>
        ) : null}
      </AnimatePresence>
    </div>
  );
};
