import { z } from "zod";

const baseTerminalTargetSchema = z.object({
  kind: z.enum(["project", "projectEnvironment", "service"]),
  projectId: z.string().trim().min(1),
  environmentSlug: z.string().trim().min(1),
  serviceId: z.string().trim().min(1).nullish(),
});

const terminalTargetSchema = baseTerminalTargetSchema.superRefine((value, ctx) => {
  if (value.kind === "service" && !value.serviceId) {
    ctx.addIssue({
      code: "custom",
      path: ["serviceId"],
      message: "Service terminal targets require a service id",
    });
  }
});

const serviceTerminalTargetSchema = baseTerminalTargetSchema.extend({
  kind: z.literal("service"),
  serviceId: z.string().trim().min(1),
});

const terminalSizeSchema = {
  cols: z.number().int().min(2).max(500),
  rows: z.number().int().min(2).max(200),
};

export const terminalCreateSessionRequestSchema = z.object({
  target: terminalTargetSchema,
  ...terminalSizeSchema,
});

// Mirrors `LaunchProfileName` on the Rust side.
const launchProfileNameSchema = z
  .string()
  .regex(/^[a-z0-9][a-z0-9_-]{0,31}$/, "Invalid launch profile name");

export const terminalLaunchServiceRequestSchema = z.object({
  target: serviceTerminalTargetSchema,
  ...terminalSizeSchema,
  profile: launchProfileNameSchema.nullish(),
});

export const terminalWriteRequestSchema = z.object({
  sessionId: z.string().trim().min(1),
  data: z.string(),
});

export const terminalResizeRequestSchema = z.object({
  sessionId: z.string().trim().min(1),
  ...terminalSizeSchema,
});

export const terminalSessionRequestSchema = z.object({
  sessionId: z.string().trim().min(1),
});
