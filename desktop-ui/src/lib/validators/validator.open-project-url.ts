import { z } from "zod";

export const openProjectUrlRequestSchema = z.object({
  url: z.string().trim().url("Project URL must be valid").regex(/^https?:\/\//, {
    message: "Project URL must use http:// or https://",
  }),
  browser: z.enum(["default", "helium", "arc", "chrome", "safari", "firefox", "brave"]),
});

export type OpenProjectUrlInput = z.infer<typeof openProjectUrlRequestSchema>;
