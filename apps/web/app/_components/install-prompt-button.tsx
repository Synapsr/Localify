"use client";

import { AnimatePresence, motion } from "motion/react";
import { useTranslations } from "next-intl";
import { useEffect, useState } from "react";
import { TextMorph } from "torph/react";

import { CheckIcon, CopyIcon } from "@/components/icons";
import { buttonVariants } from "@/components/ui/button-variants";
import { cn } from "@/lib/utils";

import { CLI_INSTALL_COMMAND } from "@/app/download-links";

/* What a person pastes into their coding agent. Kept in English: agents read it, not visitors. */
export const INSTALL_PROMPT = `Install Localify on this machine and connect yourself to it.

1. Read https://localify.dev/llms.txt.
2. Run \`${CLI_INSTALL_COMMAND}\`. It asks for my administrator password once: if you can't show me that prompt, ask me to run the command myself.
3. On macOS, run \`localify trust\` so browsers trust Localify's HTTPS. macOS asks me to confirm.
4. Run \`localify agent install\` if you are not connected yet, then tell me to restart you so the Localify MCP tools load.
5. After the restart, start this project with Localify and give me its URL.`;

type Props = {
  source: string;
  variant?: "glow" | "outline";
  size?: "default" | "lg";
  className?: string;
};

export const InstallPromptButton = ({ source, variant = "glow", size = "lg", className }: Props) => {
  const t = useTranslations("InstallPrompt");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2200);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(INSTALL_PROMPT);
      setCopied(true);
    } catch {
      // Clipboard can be blocked (permissions, insecure context): show the prompt instead.
      window.prompt(t("fallback"), INSTALL_PROMPT);
    }
  };

  return (
    <button
      aria-live="polite"
      className={cn(buttonVariants({ variant, size }), className)}
      data-umami-event="install_prompt_copied"
      data-umami-event-source={source}
      onClick={copy}
      type="button"
    >
      <MorphLabel icon={copied ? "check" : "copy"} text={copied ? t("copied") : t("cta")} />
    </button>
  );
};

/* The button's label: Torph morphs the text and eases the width; the icon crossfades. */
const MorphLabel = ({ text, icon }: { text: string; icon: "copy" | "check" }) => {
  const Icon = icon === "check" ? CheckIcon : CopyIcon;

  return (
    <span className="inline-flex items-center gap-2 whitespace-nowrap">
      <span className="relative inline-flex size-4 shrink-0">
        <AnimatePresence initial={false} mode="popLayout">
          <motion.span
            animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
            className="absolute inset-0 inline-flex"
            exit={{ opacity: 0, scale: 0.5, filter: "blur(4px)" }}
            initial={{ opacity: 0, scale: 0.5, filter: "blur(4px)" }}
            key={icon}
            transition={{ duration: 0.2 }}
          >
            <Icon className="h-4 w-4" />
          </motion.span>
        </AnimatePresence>
      </span>
      <TextMorph>{text}</TextMorph>
    </span>
  );
};
