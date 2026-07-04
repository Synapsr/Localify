import { z } from "zod";

export const addProjectServiceRequestSchema = z.object({
  projectId: z.string().trim().min(1),
  domain: z.string().trim().min(1, "Domain is required"),
  target: z.string().trim().min(1, "Target is required"),
  serviceName: z.string().trim().min(1).nullable().optional(),
  servicePath: z.string().trim().nullable().optional(),
  command: z.array(z.string().trim().min(1)).nullable().optional(),
});

export type AddProjectServiceRequestInput = z.infer<typeof addProjectServiceRequestSchema>;
