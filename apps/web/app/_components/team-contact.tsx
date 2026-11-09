"use client";

import { useLocale, useTranslations } from "next-intl";
import { type FormEvent, useState } from "react";

import { buttonVariants } from "@/components/ui/button-variants";
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { usePathname } from "@/i18n/navigation";
import { trackPlanClick } from "@/lib/track";
import { cn } from "@/lib/utils";

const SUPPORT_EMAIL = "support@localify.dev";

type Status = "idle" | "sending" | "sent" | "failed";

type Props = {
  label: string;
  seats: number;
  source: string;
  className?: string;
};

// The Team card's button: it tells the team about the click, then opens a contact
// form, pre-filled for the card's seats, that posts to /api/contact. When that
// fails, the form offers the same message by email.
export const TeamContactButton = ({ label, seats, source, className }: Props) => {
  const t = useTranslations("TeamContact");
  const locale = useLocale();
  const page = usePathname();
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<Status>("idle");
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [company, setCompany] = useState("");
  const [size, setSize] = useState(String(seats));
  // The message follows the team size until the visitor edits it.
  const [editedMessage, setEditedMessage] = useState<string | null>(null);
  const sizeNumber = Number(size);
  const message =
    editedMessage ??
    t("messageTemplate", { size: Number.isInteger(sizeNumber) && sizeNumber > 0 ? sizeNumber : seats });

  const changeOpen = (next: boolean) => {
    setOpen(next);
    // A sent request leaves an empty form for the next one; a draft is kept.
    if (!next && status === "sent") {
      setStatus("idle");
      setName("");
      setEmail("");
      setCompany("");
      setSize(String(seats));
      setEditedMessage(null);
    }
  };

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setStatus("sending");
    try {
      const response = await fetch("/api/contact", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(Object.fromEntries(new FormData(event.currentTarget))),
      });
      setStatus(response.ok ? "sent" : "failed");
    } catch {
      setStatus("failed");
    }
  };

  const signature = [name, company].filter(Boolean).join(", ");
  const mailBody = signature ? `${message}\n\n${signature}` : message;
  const mailto = `mailto:${SUPPORT_EMAIL}?subject=${encodeURIComponent(t("mailSubject"))}&body=${encodeURIComponent(mailBody)}`;
  const labelClass = "text-[13.5px] font-medium text-foreground";
  const fieldClass = "h-10 rounded-xl px-3";

  return (
    <Dialog onOpenChange={changeOpen} open={open}>
      <DialogTrigger
        className={className}
        data-umami-event="cloud_team_clicked"
        data-umami-event-source={source}
        onClick={() => trackPlanClick({ plan: "team", locale, page })}
      >
        {label}
      </DialogTrigger>
      <DialogContent
        className="max-h-[calc(100dvh-2rem)] overflow-y-auto rounded-3xl sm:max-w-lg"
        closeLabel={t("close")}
      >
        {status === "sent" ? (
          <>
            <DialogHeader className="pr-14">
              <DialogTitle className="text-[22px] tracking-[-0.02em]">{t("successTitle")}</DialogTitle>
              <DialogDescription className="text-[14.5px] leading-[1.55]">
                {t("successBody", { email })}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter className="pb-6">
              <DialogClose autoFocus className={cn(buttonVariants({ size: "lg" }), "w-full sm:ml-auto sm:w-auto")}>
                {t("close")}
              </DialogClose>
            </DialogFooter>
          </>
        ) : (
          <form className="grid gap-6" onSubmit={submit}>
            <DialogHeader className="pr-14">
              <DialogTitle className="text-[22px] tracking-[-0.02em]">{t("title")}</DialogTitle>
              <DialogDescription className="text-[14.5px] leading-[1.55]">{t("description")}</DialogDescription>
            </DialogHeader>

            <DialogBody className="grid gap-4 sm:grid-cols-2">
              <label className="grid gap-1.5">
                <span className={labelClass}>{t("name")}</span>
                <Input
                  autoComplete="name"
                  className={fieldClass}
                  maxLength={100}
                  name="name"
                  onChange={(event) => setName(event.target.value)}
                  required
                  value={name}
                />
              </label>
              <label className="grid gap-1.5">
                <span className={labelClass}>{t("email")}</span>
                <Input
                  autoComplete="email"
                  className={fieldClass}
                  maxLength={254}
                  name="email"
                  onChange={(event) => setEmail(event.target.value)}
                  required
                  type="email"
                  value={email}
                />
              </label>
              <label className="grid gap-1.5">
                <span className={labelClass}>
                  {t("company")} <span className="font-normal text-muted-foreground">({t("optional")})</span>
                </span>
                <Input
                  autoComplete="organization"
                  className={fieldClass}
                  maxLength={100}
                  name="company"
                  onChange={(event) => setCompany(event.target.value)}
                  value={company}
                />
              </label>
              <label className="grid gap-1.5">
                <span className={labelClass}>{t("size")}</span>
                <Input
                  className={fieldClass}
                  inputMode="numeric"
                  max={100000}
                  min={1}
                  name="size"
                  onChange={(event) => setSize(event.target.value)}
                  step={1}
                  type="number"
                  value={size}
                />
              </label>
              <label className="grid gap-1.5 sm:col-span-2">
                <span className={labelClass}>{t("message")}</span>
                <Textarea
                  className="min-h-28 rounded-xl px-3"
                  maxLength={2000}
                  name="message"
                  onChange={(event) => setEditedMessage(event.target.value)}
                  value={message}
                />
              </label>

              <input name="plan" type="hidden" value="team" />
              <input name="locale" type="hidden" value={locale} />
              <input name="page" type="hidden" value={page} />
              {/* The honeypot: hidden from people and screen readers, filled by bots. */}
              <div aria-hidden="true" className="absolute -left-[10000px] h-px w-px overflow-hidden">
                <label>
                  Website
                  <input autoComplete="off" name="website" tabIndex={-1} type="text" />
                </label>
              </div>

              <div aria-live="polite" className="sm:col-span-2">
                {status === "failed" ? (
                  <div className="space-y-3 rounded-2xl bg-muted/60 p-4 ring-1 ring-border/60">
                    <p className="text-[14px] leading-[1.55] text-pretty text-foreground">{t("errorBody")}</p>
                    <a
                      className={cn(buttonVariants({ variant: "outline", size: "lg" }), "w-full whitespace-normal")}
                      href={mailto}
                    >
                      {t("emailUs")}
                    </a>
                  </div>
                ) : null}
              </div>
            </DialogBody>

            <DialogFooter className="pb-6">
              <button
                className={cn(buttonVariants({ size: "lg" }), "w-full sm:ml-auto sm:w-auto")}
                disabled={status === "sending"}
                type="submit"
              >
                {status === "sending" ? t("sending") : t("submit")}
              </button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
};
