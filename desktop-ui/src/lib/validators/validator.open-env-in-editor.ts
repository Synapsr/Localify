import { z } from "zod";

export const openEnvInEditorRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  environmentSlug: z.string().trim().min(1, "Environment slug is required"),
  serviceId: z.string().trim().min(1, "Service id is required").nullable().optional(),
  relativePath: z.string().trim().min(1, "Environment file path is required").nullable().optional(),
});

export type OpenEnvInEditorInput = z.infer<typeof openEnvInEditorRequestSchema>;
