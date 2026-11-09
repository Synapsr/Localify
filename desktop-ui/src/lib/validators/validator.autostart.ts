import { z } from "zod";

export const updateAutostartRequestSchema = z.object({
  enabled: z.boolean(),
});

export type UpdateAutostartInput = z.infer<typeof updateAutostartRequestSchema>;

export const updateSettingsRequestSchema = z.object({
  automatic: z.boolean(),
});
