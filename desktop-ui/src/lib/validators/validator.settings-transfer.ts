import { z } from "zod";

const absolutePathSchema = z
  .string()
  .trim()
  .min(1, "A file path is required")
  .refine((value) => value.startsWith("/"), "The file path must be absolute");

const projectIdsSchema = z.array(z.string().trim().min(1)).default([]);

export const settingsExportRequestSchema = z.object({
  filePath: absolutePathSchema,
  mode: z.union([z.literal("all"), z.literal("projects")]),
  projectIds: projectIdsSchema,
});

export const settingsImportPreviewRequestSchema = z.object({
  filePath: absolutePathSchema,
});

export const settingsImportRequestSchema = z.object({
  filePath: absolutePathSchema,
  mode: z.union([z.literal("replaceAll"), z.literal("projects")]),
  projectIds: projectIdsSchema,
});

export type SettingsExportInput = z.infer<typeof settingsExportRequestSchema>;
export type SettingsImportPreviewInput = z.infer<typeof settingsImportPreviewRequestSchema>;
export type SettingsImportInput = z.infer<typeof settingsImportRequestSchema>;
