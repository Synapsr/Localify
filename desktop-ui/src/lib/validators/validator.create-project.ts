import { z } from "zod";

export const projectDomainEntrySchema = z.object({
  domain: z.string().trim(),
  target: z.string().trim(),
  wildcard: z.boolean(),
});

export type ProjectDomainEntry = z.infer<typeof projectDomainEntrySchema>;

export const createProjectSchema = z
  .object({
    name: z.string().trim().min(1, "Project name is required"),
    domains: z.array(projectDomainEntrySchema).min(1),
  })
  .superRefine((value, ctx) => {
    value.domains.forEach((entry, index) => {
      const hasDraft = entry.domain.length > 0 || entry.target.length > 0;
      const isPrimary = index === 0;

      if (isPrimary && !hasDraft) {
        return;
      }

      if (entry.domain.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["domains", index, "domain"],
          message: isPrimary
            ? "Domain is required when a target is set"
            : "Domain is required",
        });
      }
      if (entry.target.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["domains", index, "target"],
          message: isPrimary
            ? "Target is required when a domain is set"
            : "Target is required",
        });
      }
    });
  });

export type CreateProjectInput = z.infer<typeof createProjectSchema>;

export const createProjectWithDomainsRequestSchema = z.object({
  name: z.string().trim().min(1, "Project name is required"),
  domains: z
    .array(
      z.object({
        domain: z.string().trim().min(1, "Domain is required"),
        wildcard: z.boolean(),
        routes: z
          .array(z.string().trim().min(1, "Route target is required"))
          .min(1, "At least one route is required"),
      }),
    )
    .default([]),
});

export const registerDomainRequestSchema = z.object({
  domain: z.string().trim().min(1, "Domain is required"),
  wildcard: z.boolean(),
  routes: z
    .array(z.string().trim().min(1, "Route target is required"))
    .min(1, "At least one route is required"),
  projectId: z.string().trim().min(1).nullable().optional(),
});

export const updateDomainRequestSchema = registerDomainRequestSchema.extend({
  currentDomain: z.string().trim().min(1, "Current domain is required"),
  currentWildcard: z.boolean(),
});

export function emptyDomainEntry(): ProjectDomainEntry {
  return { domain: "", target: "", wildcard: false };
}

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
