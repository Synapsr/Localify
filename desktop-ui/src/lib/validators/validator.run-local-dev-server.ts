import { z } from "zod";

export const runLocalDevServerRequestSchema = z.object({
  rootPath: z.string().trim().min(1, "Project path is required"),
  target: z.string().trim().min(1, "Route target is required"),
});

export type RunLocalDevServerInput = z.infer<typeof runLocalDevServerRequestSchema>;
