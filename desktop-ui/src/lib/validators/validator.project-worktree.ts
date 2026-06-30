import { z } from "zod";

export const discoverProjectWorktreesRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
});

export const attachProjectWorktreeRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  rootPath: z.string().trim().min(1, "Worktree path is required"),
  primaryRootPath: z.string().trim().min(1).nullable().optional(),
});

export const environmentFileCopyRequestSchema = z.object({
  projectId: z.string().trim().min(1, "Project id is required"),
  sourceEnvironmentSlug: z.string().trim().min(1, "Source environment is required"),
  targetEnvironmentSlug: z.string().trim().min(1, "Target environment is required"),
  relativePaths: z.array(z.string().trim().min(1, "Environment file path is required")),
  overwrite: z.boolean(),
});

export type DiscoverProjectWorktreesInput = z.infer<typeof discoverProjectWorktreesRequestSchema>;
export type AttachProjectWorktreeInput = z.infer<typeof attachProjectWorktreeRequestSchema>;
export type EnvironmentFileCopyInput = z.infer<typeof environmentFileCopyRequestSchema>;
