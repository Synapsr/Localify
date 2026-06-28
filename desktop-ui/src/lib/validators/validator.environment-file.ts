import { z } from "zod";

export const environmentFileRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  environmentSlug: z.string().trim().min(1, "Environment slug is required"),
  serviceId: z.string().trim().min(1, "Service id is required").nullable().optional(),
  relativePath: z.string().trim().min(1, "Environment file path is required").nullable().optional(),
});

export const saveEnvironmentFileRequestSchema = environmentFileRequestSchema.extend({
  content: z.string(),
  expectedModifiedAt: z.number().int().nonnegative().nullable().optional(),
});

export type EnvironmentFileInput = z.infer<typeof environmentFileRequestSchema>;
export type SaveEnvironmentFileInput = z.infer<typeof saveEnvironmentFileRequestSchema>;
