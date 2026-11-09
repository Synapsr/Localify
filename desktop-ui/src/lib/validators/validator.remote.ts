import { z } from "zod";

// Remote machines (docs/remote-machines.md, "Daemon").

const machineIdSchema = z.string().trim().min(1, "Machine id is required");

export const remoteSettingsRequestSchema = z.object({
  share: z.boolean().optional(),
  control: z.boolean().optional(),
  backgroundStarts: z.boolean().optional(),
});

export const remoteMachineRequestSchema = z.object({
  machineId: machineIdSchema,
});

export const remotePairAnswerRequestSchema = z.object({
  pairingId: z.string().trim().min(1, "Pairing id is required"),
  accept: z.boolean(),
});

export const remoteAccessWantsSchema = z.enum(["control", "backgroundStarts"]);

export const remoteAccessAnswerRequestSchema = z.object({
  requestId: z.string().trim().min(1, "Request id is required"),
  allow: z.boolean(),
  suppress: z.boolean(),
});

export const remoteStreamRequestSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("events"), access: z.boolean().optional() }),
  z.object({ kind: z.literal("pair"), machineId: machineIdSchema }),
  z.object({ kind: z.literal("watch"), machineId: machineIdSchema }),
  z.object({
    kind: z.literal("follow"),
    machineId: machineIdSchema,
    runId: z.string().trim().min(1, "Run id is required"),
    offset: z.number().int().nonnegative().nullable(),
  }),
  z.object({
    kind: z.literal("access"),
    machineId: machineIdSchema,
    wants: remoteAccessWantsSchema,
  }),
]);
