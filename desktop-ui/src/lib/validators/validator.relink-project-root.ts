import { z } from "zod";

export const relinkProjectRootRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  rootPath: z.string().trim().min(1, "Folder path is required"),
});

export type RelinkProjectRootInput = z.infer<typeof relinkProjectRootRequestSchema>;
