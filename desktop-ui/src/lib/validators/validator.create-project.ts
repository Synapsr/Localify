import { z } from "zod";

export const createProjectSchema = z
  .object({
    name: z.string().trim().min(1, "Project name is required"),
    appDomain: z.string().trim(),
    appTarget: z.string().trim(),
    includeApi: z.boolean(),
    apiDomain: z.string().trim(),
    apiTarget: z.string().trim(),
  })
  .superRefine((value, ctx) => {
    const hasAppDraft = value.appDomain.length > 0 || value.appTarget.length > 0;
    if (hasAppDraft && value.appDomain.length === 0) {
      ctx.addIssue({
        code: "custom",
        path: ["appDomain"],
        message: "Domain is required when a target is set",
      });
    }
    if (hasAppDraft && value.appTarget.length === 0) {
      ctx.addIssue({
        code: "custom",
        path: ["appTarget"],
        message: "Target is required when a domain is set",
      });
    }

    if (value.includeApi) {
      if (value.apiDomain.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["apiDomain"],
          message: "API domain is required",
        });
      }
      if (value.apiTarget.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["apiTarget"],
          message: "API target is required",
        });
      }
    }
  });

export type CreateProjectInput = z.infer<typeof createProjectSchema>;

export function normalizeDomain(value: string, suffix: string): string {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return `example${suffix}`;
  }
  return trimmed.endsWith(suffix) ? trimmed : `${trimmed}${suffix}`;
}

export function normalizeSuffix(suffix: string): string {
  return suffix.startsWith(".") ? suffix : `.${suffix || "localify"}`;
}

export function slugifyProjectName(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
