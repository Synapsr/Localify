import { z } from "zod";

export const removeProjectRequestSchema = z.object({
  id: z.string().trim().min(1, "Project id is required"),
  deleteDomains: z.boolean().default(true),
});

export type RemoveProjectInput = z.infer<typeof removeProjectRequestSchema>;
