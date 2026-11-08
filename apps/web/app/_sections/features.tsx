import { getTranslations } from "next-intl/server";
import Image from "next/image";

import { FadeIn, SectionHeading, sectionClass } from "@/app/site-ui";
import { FileTextSolidIcon, HexagonSolidIcon, LockIcon } from "@/components/icons";

const cardKeys = ["https", "profiles", "stack"] as const;

const cardIcons = {
  https: LockIcon,
  profiles: FileTextSolidIcon,
  stack: HexagonSolidIcon,
} as const;

const cardImages = {
  https: "/sky/ui.jpg",
  profiles: "/sky/agents.jpg",
  stack: "/sky/urls.jpg",
} as const;

export const Features = async () => {
  const t = await getTranslations("Features");

  return (
    <section className={sectionClass} id="features">
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

      <div className="mt-14 grid gap-4 md:mt-20 md:grid-cols-3 md:gap-5">
        {cardKeys.map((key, index) => {
          const Icon = cardIcons[key];
          return (
            <FadeIn key={key} delay={0.05 + index * 0.08}>
              <article className="group relative flex h-full flex-col gap-6 overflow-hidden rounded-3xl border border-white/10 bg-surface p-7 shadow-soft md:p-8">
                <Image
                  alt=""
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-0 h-full w-full object-cover"
                  fill
                  priority={false}
                  sizes="(min-width: 768px) 33vw, 100vw"
                  src={cardImages[key]}
                />
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-0"
                  style={{
                    backgroundImage:
                      "linear-gradient(to bottom, rgba(0,0,0,0.08) 5%, rgba(0,0,0,0.55) 55%, rgba(0,0,0,0.72) 100%)",
                  }}
                />
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-0 rounded-[inherit] opacity-0 transition-opacity duration-500 ease-out group-hover:opacity-100"
                  style={{
                    backgroundImage:
                      "radial-gradient(circle at top left, rgba(255,255,255,0.18) 0%, transparent 55%)",
                  }}
                />
                <span
                  aria-hidden="true"
                  className="relative inline-flex h-11 w-11 items-center justify-center rounded-2xl border border-white/15 bg-white/10 text-white/85 backdrop-blur-sm transition-colors duration-300 group-hover:text-white"
                >
                  <Icon className="h-[18px] w-[18px]" strokeWidth={2} />
                </span>
                <h3 className="relative text-[20px] leading-[1.2] font-medium tracking-tight text-balance text-white md:text-[22px]">
                  {t(`cards.${key}.title`)}
                </h3>
                <p className="relative text-[14.5px] leading-[1.55] text-pretty text-white/75">
                  {t(`cards.${key}.body`)}
                </p>
              </article>
            </FadeIn>
          );
        })}
      </div>
    </section>
  );
};
