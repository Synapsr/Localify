import { z } from "zod";

export const removeProjectEnvironmentRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  environmentSlug: z.string().trim().min(1, "Environment slug is required"),
});

export type RemoveProjectEnvironmentInput = z.infer<
  typeof removeProjectEnvironmentRequestSchema
>;
