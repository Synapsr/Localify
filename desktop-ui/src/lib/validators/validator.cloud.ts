import { z } from "zod";

// Slug rules from docs/public-urls.md, section "Names".

export const publicUrlSlugSchema = z
  .string()
  .trim()
  .min(3, "Use 3 to 32 characters.")
  .max(32, "Use 3 to 32 characters.")
  .regex(/^[a-z][a-z0-9-]*$/, "Use a-z, 0-9 and hyphens, starting with a letter.")
  .refine((slug) => !slug.endsWith("-"), "Do not end with a hyphen.")
  .refine((slug) => !slug.includes("--"), "Do not put two hyphens in a row.");

export const publicUrlPasswordSchema = z.string().trim().min(8, "Use at least 8 characters.");

const protectionModeSchema = z.enum(["password", "public"]);

const idSchema = z.string().trim().min(1, "Public URL id is required");

export const pollCloudSignInRequestSchema = z.object({
  deviceCode: z.string().trim().min(1, "Device code is required"),
});

export const createPublicUrlRequestSchema = z
  .object({
    localDomain: z.string().trim().min(1, "Local domain is required"),
    wildcard: z.boolean(),
    slug: publicUrlSlugSchema.optional(),
    mode: protectionModeSchema,
    password: publicUrlPasswordSchema.optional(),
  })
  .refine((request) => request.mode === "password" || request.password === undefined, {
    message: "A URL open to anyone has no password.",
    path: ["password"],
  });

export const publicUrlRequestSchema = z.object({ id: idSchema });

export const setPublicUrlProtectionRequestSchema = z
  .object({
    id: idSchema,
    mode: protectionModeSchema,
    password: publicUrlPasswordSchema.optional(),
  })
  .refine((request) => request.mode === "password" || request.password === undefined, {
    message: "A URL open to anyone has no password.",
    path: ["password"],
  });

/** Empty slug and password mean "generate one". */
export const createPublicUrlFormSchema = z
  .object({
    wildcard: z.boolean(),
    slug: z.string(),
    mode: protectionModeSchema,
    password: z.string(),
    confirmPublic: z.boolean(),
  })
  .superRefine((value, ctx) => {
    addOptionalIssue(ctx, ["slug"], publicUrlSlugSchema, value.slug);
    if (value.mode === "password") {
      addOptionalIssue(ctx, ["password"], publicUrlPasswordSchema, value.password);
    }
    if (value.mode === "public" && !value.confirmPublic) {
      ctx.addIssue({
        code: "custom",
        path: ["confirmPublic"],
        message: "Confirm that anyone with the link may open it.",
      });
    }
  });

export type CreatePublicUrlFormInput = z.infer<typeof createPublicUrlFormSchema>;

/** Empty means "generate one". */
export const publicUrlPasswordFormSchema = z
  .object({ password: z.string() })
  .superRefine((value, ctx) => {
    addOptionalIssue(ctx, ["password"], publicUrlPasswordSchema, value.password);
  });

export type PublicUrlPasswordFormInput = z.infer<typeof publicUrlPasswordFormSchema>;

/** Optional text: checks it against `schema` only when it is not blank. */
function addOptionalIssue(
  ctx: z.RefinementCtx,
  path: string[],
  schema: z.ZodType<string>,
  value: string,
) {
  if (value.trim().length === 0) return;
  const result = schema.safeParse(value);
  if (!result.success) {
    ctx.addIssue({ code: "custom", path, message: result.error.issues[0]?.message });
  }
}

/** Blank text becomes undefined, so the request leaves the field out. */
export function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}
