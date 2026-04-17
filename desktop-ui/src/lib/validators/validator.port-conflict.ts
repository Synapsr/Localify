import { z } from "zod";

export const killPortConflictRequestSchema = z.object({
  protocol: z.enum(["http", "https", "dns"]),
  port: z.number().int().min(1).max(65535),
  pid: z.number().int().positive().max(4294967295),
});

export type KillPortConflictInput = z.infer<typeof killPortConflictRequestSchema>;
