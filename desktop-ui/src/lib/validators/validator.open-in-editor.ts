import { z } from "zod";

export const openInEditorRequestSchema = z.object({
  path: z.string().trim().min(1, "Project path is required"),
});

export type OpenInEditorInput = z.infer<typeof openInEditorRequestSchema>;
