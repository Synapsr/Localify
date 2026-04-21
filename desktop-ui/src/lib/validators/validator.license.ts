import { z } from "zod";

export const keygenSettingsUpdateRequestSchema = z.object({
  apiUrl: z.string().trim().url("A valid Keygen API URL is required"),
  account: z.string().trim().min(1, "A Keygen account is required"),
  product: z.string().trim().optional().nullable(),
  publicKey: z.string().trim().optional().nullable(),
});

export const activateLicenseRequestSchema = z.object({
  key: z.string().trim().min(1, "License key is required"),
});

export const importOfflineLicenseRequestSchema = z.object({
  certificate: z
    .string()
    .trim()
    .min(1, "An offline license certificate is required"),
  key: z.string().trim().optional().nullable(),
});
