"use client";

import * as motion from "motion/react-client";
import { useTranslations } from "next-intl";
import { useEffect, useRef, type ReactNode } from "react";

import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";

type VideoSrc = {
  mp4?: string;
  webm?: string;
  poster?: string;
};

type MediaRowProps = {
  title: ReactNode;
  body: ReactNode;
  reverse?: boolean;
  video?: VideoSrc;
};

const ease = [0.22, 1, 0.36, 1] as const;

const textContainer = {
  hidden: {},
  show: {
    transition: {
      staggerChildren: 0.08,
      delayChildren: 0.05,
    },
  },
};

const textItem = {
  hidden: { opacity: 0, y: 12 },
  show: {
    opacity: 1,
    y: 0,
    transition: { duration: 0.55, ease },
  },
};

const mediaReveal = {
  hidden: { opacity: 0, y: 16, scale: 0.985 },
  show: {
    opacity: 1,
    y: 0,
    scale: 1,
    transition: { duration: 0.7, ease, delay: 0.12 },
  },
};

const videos = [
  { mp4: "/videos/flow-1-web.mp4" },
  { mp4: "/videos/flow-2-web.mp4" },
  { mp4: "/videos/flow-3-web.mp4" },
  { mp4: "/videos/flow-4-web.mp4" },
  { mp4: "/videos/flow-5-web.mp4" },
] as const;

export const HowItWorks = () => {
  const t = useTranslations("HowItWorks");
  const ta = useTranslations("Agents");
  const steps = t.raw("steps") as Array<{ title: string; body: string }>;

  const rows: Array<{ key: string; title: string; body: string; video?: VideoSrc }> = steps.map(
    (step, index) => ({
      key: step.title,
      title: step.title,
      body: step.body,
      video: videos[index],
    }),
  );

  // Agents are one of the ways into a project — drop them inline, right
  // after the folder-import and CLI steps.
  rows.splice(2, 0, {
    key: "agents",
    title: ta("title"),
    body: ta("description"),
    video: { mp4: "/videos/agent.mp4" },
  });

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
          description={t("description")}
        />
      </FadeIn>

      <div className="mt-20 space-y-24 md:mt-28 md:space-y-36">
        {rows.map((row, index) => (
          <MediaRow
            key={row.key}
            reverse={index % 2 === 1}
            video={row.video}
            title={row.title}
            body={row.body}
          />
        ))}
      </div>
    </section>
  );
};

const MediaRow = ({ title, body, reverse, video }: MediaRowProps) => {
  const hasVideo = !!video?.mp4 || !!video?.webm;
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.intersectionRatio < 1) return;
        document.querySelectorAll("video").forEach((other) => {
          if (other !== el) other.pause();
        });
        el.play().catch(() => {});
      },
      { threshold: 1 },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="grid items-center gap-10 md:grid-cols-2 md:gap-16">
      <motion.div
        className={reverse ? "md:order-2" : ""}
        initial="hidden"
        variants={textContainer}
        viewport={{ once: true, amount: 0.4 }}
        whileInView="show"
      >
        <div className="space-y-5">
          <motion.h3
            className="text-[32px] leading-[1.05] font-medium tracking-[-0.03em] text-balance text-foreground md:text-[40px]"
            variants={textItem}
          >
            {title}
          </motion.h3>
          <motion.p
            className="max-w-lg text-[16.5px] leading-[1.55] text-pretty text-muted-foreground"
            variants={textItem}
          >
            {body}
          </motion.p>
        </div>
      </motion.div>

      <motion.div
        className={reverse ? "md:order-1" : ""}
        initial="hidden"
        variants={mediaReveal}
        viewport={{ once: true, amount: 0.4 }}
        whileInView="show"
      >
        <div className="relative aspect-4/3 overflow-hidden rounded-2xl border border-border bg-muted">
          {hasVideo ? (
            <video
              ref={videoRef}
              className="h-full w-full object-cover ring-1 ring-black/10 dark:ring-white/10"
              loop
              muted
              playsInline
              poster={video?.poster}
              preload="metadata"
            >
              {video?.webm ? <source src={video.webm} type="video/webm" /> : null}
              {video?.mp4 ? <source src={video.mp4} type="video/mp4" /> : null}
            </video>
          ) : null}
        </div>
      </motion.div>
    </div>
  );
};
