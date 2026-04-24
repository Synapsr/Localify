import { z } from "zod";

export const stopLocalDevServerRequestSchema = z.object({
  target: z.string().trim().min(1, "Route target is required"),
});

export type StopLocalDevServerInput = z.infer<typeof stopLocalDevServerRequestSchema>;
