"use client";

import { RiGlobalLine } from "@remixicon/react";
import { useLocale, useTranslations } from "next-intl";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { usePathname, useRouter } from "@/i18n/navigation";
import { routing, type Locale } from "@/i18n/routing";
import { cn } from "@/lib/utils";

const localizedPathPattern = new RegExp(
  `^/(${routing.locales.map((locale) => locale.replace("-", "\\-")).join("|")})(?=/|$)`,
);

const removeLocalePrefix = (pathname: string) => {
  let unprefixedPathname = pathname;

  while (localizedPathPattern.test(unprefixedPathname)) {
    unprefixedPathname = unprefixedPathname.replace(localizedPathPattern, "");
  }

  return unprefixedPathname || "/";
};

const getLocaleFromPathname = (pathname: string) => {
  const match = pathname.match(localizedPathPattern);
  const locale = match?.[1];

  return locale && routing.locales.includes(locale as Locale) ? (locale as Locale) : null;
};

const localeLabels: Record<Locale, { native: string; short: string }> = {
  en: { native: "English", short: "EN" },
  fr: { native: "Français", short: "FR" },
  es: { native: "Español", short: "ES" },
  de: { native: "Deutsch", short: "DE" },
  "pt-BR": { native: "Português", short: "PT" },
  ja: { native: "日本語", short: "JA" },
  ko: { native: "한국어", short: "KO" },
  "zh-CN": { native: "中文", short: "ZH" },
  ru: { native: "Русский", short: "RU" },
};

export const FooterLanguageSelect = () => {
  const t = useTranslations("Footer");
  const fallbackLocale = useLocale() as Locale;
  const pathname = usePathname();
  const router = useRouter();
  const currentLocale = getLocaleFromPathname(pathname) ?? fallbackLocale;
  const currentLabel = localeLabels[currentLocale];

  const handleSelect = (next: string) => {
    if (next === currentLocale) return;

    const unprefixedPathname = removeLocalePrefix(pathname);
    const hash = window.location.hash;
    const nextPath = hash ? `${unprefixedPathname}${hash}` : unprefixedPathname;

    router.replace(nextPath, { locale: next as Locale });
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={t("language")}
        className={cn(
          "inline-flex h-8 items-center gap-1.5 rounded-full border border-border bg-background px-3 text-[12px] font-medium text-foreground outline-none transition-colors duration-150",
          "hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/40",
          "data-[popup-open]:bg-muted",
        )}
      >
        <RiGlobalLine className="size-3.5 text-muted-foreground" aria-hidden />
        <span>{currentLabel.native}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        side="top"
        sideOffset={6}
        className="min-w-[160px]"
      >
        <DropdownMenuRadioGroup value={currentLocale} onValueChange={handleSelect}>
          {routing.locales.map((locale) => {
            const label = localeLabels[locale];
            const isActive = locale === currentLocale;

            return (
              <DropdownMenuRadioItem
                key={locale}
                value={locale}
                aria-label={t("switchLanguage", { locale: label.native })}
                className="pr-8 pl-2"
              >
                <span className="flex flex-1 items-center justify-between gap-3">
                  <span className={cn(isActive && "font-medium")}>{label.native}</span>
                  <span className="text-[11px] tabular-nums text-muted-foreground">
                    {label.short}
                  </span>
                </span>
              </DropdownMenuRadioItem>
            );
          })}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};
