"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { PublicUrlProtectionModeDto, PublicUrlWithPasswordDto } from "@/types/localify";

import { Button } from "@/components/ui/button";
import { DialogBody, DialogClose, DialogFooter } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { useAppForm } from "@/hooks/form/form";
import { cloudMutations } from "@/lib/queries";
import { slugExample } from "@/lib/util.cloud";
import { cn } from "@/lib/utils";
import {
  type CreatePublicUrlFormInput,
  createPublicUrlFormSchema,
  optionalText,
} from "@/lib/validators/validator.cloud";

interface PublicUrlCreateFormProps {
  localDomain: string;
  /** Offer the wildcard switch: only a project's namespace has environments under it. */
  allowWildcard: boolean;
  wildcardByDefault: boolean;
  onCreated: (result: PublicUrlWithPasswordDto) => void;
}

const defaultValues: CreatePublicUrlFormInput = {
  wildcard: false,
  slug: "",
  mode: "password",
  password: "",
  confirmPublic: false,
};

export const PublicUrlCreateForm = ({
  localDomain,
  allowWildcard,
  wildcardByDefault,
  onCreated,
}: PublicUrlCreateFormProps) => {
  const queryClient = useQueryClient();
  const createPublicUrl = useMutation(cloudMutations.createPublicUrl(queryClient));
  const [submitError, setSubmitError] = useState<string | null>(null);

  const form = useAppForm({
    defaultValues: { ...defaultValues, wildcard: allowWildcard && wildcardByDefault },
    validationLogic: revalidateLogic({ mode: "submit", modeAfterSubmission: "change" }),
    validators: { onDynamic: createPublicUrlFormSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);
      try {
        const result = await createPublicUrl.mutateAsync({
          localDomain,
          wildcard: allowWildcard && value.wildcard,
          slug: optionalText(value.slug),
          mode: value.mode,
          password: value.mode === "password" ? optionalText(value.password) : undefined,
        });
        onCreated(result);
      } catch (error) {
        setSubmitError(error instanceof Error ? error.message : String(error));
      }
    },
  });

  return (
    <form.AppForm>
      <form
        noValidate
        className="space-y-5"
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <DialogBody className="space-y-5">
          {allowWildcard ? (
            <form.AppField name="wildcard">
              {(field) => (
                <div className="flex items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-foreground">Every environment</div>
                    <div className="text-xs leading-normal text-muted-foreground">
                      Each environment of the project also gets its own URL, at{" "}
                      <span className="font-mono text-foreground">
                        &lt;slug&gt;--&lt;label&gt;.localify.fyi
                      </span>
                      .
                    </div>
                  </div>
                  <Switch
                    aria-label="Every environment"
                    checked={field.state.value}
                    onCheckedChange={(checked) => field.handleChange(checked)}
                  />
                </div>
              )}
            </form.AppField>
          ) : null}

          <form.AppField name="slug">
            {(field) => (
              <field.TextField
                label="Slug (optional)"
                placeholder={slugExample(localDomain)}
                description="Leave empty to generate one."
              />
            )}
          </form.AppField>

          <form.AppField name="mode">
            {(field) => (
              <fieldset className="space-y-2">
                <legend className="mb-2 text-sm font-medium text-foreground">
                  Who can open it
                </legend>
                <ProtectionOption
                  value="password"
                  checked={field.state.value === "password"}
                  onSelect={field.handleChange}
                  title="Password (recommended)"
                  description="Each browser asks for the password, then remembers it for 7 days."
                />
                <ProtectionOption
                  value="public"
                  checked={field.state.value === "public"}
                  onSelect={field.handleChange}
                  title="Anyone with the link"
                  description="No password. Anyone who has the URL can open your dev server."
                />
              </fieldset>
            )}
          </form.AppField>

          <form.Subscribe selector={(state) => state.values.mode}>
            {(mode) =>
              mode === "password" ? (
                <form.AppField name="password">
                  {(field) => (
                    <field.TextField
                      label="Custom password (optional)"
                      placeholder="Leave empty to generate one"
                      description="At least 8 characters."
                    />
                  )}
                </form.AppField>
              ) : (
                <form.AppField name="confirmPublic">
                  {(field) => {
                    const error = firstErrorMessage(field.state.meta.errors);
                    return (
                      <div className="space-y-1">
                        <label className="flex items-start gap-2 text-sm text-foreground">
                          <input
                            type="checkbox"
                            checked={field.state.value}
                            onChange={(event) => field.handleChange(event.target.checked)}
                            onBlur={field.handleBlur}
                            aria-invalid={error ? true : undefined}
                            className="mt-0.5 size-4 shrink-0 accent-primary"
                          />
                          <span>
                            I understand that anyone with the link can open{" "}
                            <span className="font-mono">{localDomain}</span>.
                          </span>
                        </label>
                        {error ? (
                          <p role="alert" className="pl-6 text-xs text-destructive">
                            {error}
                          </p>
                        ) : null}
                      </div>
                    );
                  }}
                </form.AppField>
              )
            }
          </form.Subscribe>

          {submitError ? (
            <p role="alert" className="text-xs text-destructive">
              {submitError}
            </p>
          ) : null}
        </DialogBody>

        <DialogFooter>
          <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
          <form.SubmitButton pendingLabel="Creating…">Create public URL</form.SubmitButton>
        </DialogFooter>
      </form>
    </form.AppForm>
  );
};

interface ProtectionOptionProps {
  value: PublicUrlProtectionModeDto;
  checked: boolean;
  onSelect: (value: PublicUrlProtectionModeDto) => void;
  title: string;
  description: string;
}

const ProtectionOption = ({
  value,
  checked,
  onSelect,
  title,
  description,
}: ProtectionOptionProps) => (
  <label
    className={cn(
      "flex cursor-pointer items-start gap-3 rounded-xl border p-3 transition-colors",
      checked ? "border-foreground/25 bg-muted/50" : "border-border/60 hover:bg-muted/30",
    )}
  >
    <input
      type="radio"
      name="public-url-protection"
      value={value}
      checked={checked}
      onChange={() => onSelect(value)}
      className="mt-0.5 size-4 shrink-0 accent-primary"
    />
    <span className="min-w-0 flex-1 space-y-0.5">
      <span className="block text-sm font-medium text-foreground">{title}</span>
      <span className="block text-xs leading-normal text-muted-foreground">{description}</span>
    </span>
  </label>
);

function firstErrorMessage(errors: unknown[]): string | null {
  for (const error of errors) {
    if (typeof error === "string" && error.length > 0) return error;
    if (error && typeof error === "object" && "message" in error) {
      const message = (error as { message?: unknown }).message;
      if (typeof message === "string" && message.length > 0) return message;
    }
  }
  return null;
}
