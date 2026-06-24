import { z } from "zod";

export const folderInitServiceSchema = z.object({
  id: z.string().trim().min(1),
  label: z.string().trim().min(1),
  displayPath: z.string().trim().min(1),
  serviceName: z.string().trim().min(1),
  servicePath: z.string().trim().nullable(),
  command: z.array(z.string().trim().min(1)).nullable(),
  domain: z.string().trim(),
  target: z.string().trim(),
  enabled: z.boolean(),
  canUpdateDevScript: z.boolean(),
  updateDevScript: z.boolean(),
});

export type FolderInitServiceInput = z.infer<typeof folderInitServiceSchema>;

export const initProjectFromFolderSchema = z
  .object({
    rootPath: z.string().trim().min(1, "Select a project folder"),
    name: z.string().trim().min(1, "Project name is required"),
    services: z.array(folderInitServiceSchema).min(1),
  })
  .superRefine((value, ctx) => {
    let enabledCount = 0;

    value.services.forEach((service, index) => {
      if (!service.enabled) {
        return;
      }

      enabledCount += 1;

      if (service.domain.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["services", index, "domain"],
          message: "Domain is required",
        });
      }

      if (service.target.length === 0) {
        ctx.addIssue({
          code: "custom",
          path: ["services", index, "target"],
          message: "Target is required",
        });
      }
    });

    if (enabledCount === 0) {
      ctx.addIssue({
        code: "custom",
        path: ["services"],
        message: "Select at least one app to initialize",
      });
    }
  });

export type InitProjectFromFolderInput = z.infer<typeof initProjectFromFolderSchema>;

export const detectProjectInitPlanRequestSchema = z.object({
  rootPath: z.string().trim().min(1, "Select a project folder"),
});

export const initProjectFromFolderRequestSchema = z.object({
  rootPath: z.string().trim().min(1, "Select a project folder"),
  name: z.string().trim().min(1, "Project name is required"),
  services: z.array(
    z.object({
      id: z.string().trim().min(1),
      domain: z.string().trim(),
      target: z.string().trim(),
      enabled: z.boolean(),
      updateDevScript: z.boolean(),
      serviceName: z.string().trim().min(1).nullable().optional(),
      servicePath: z.string().trim().nullable().optional(),
      command: z.array(z.string().trim().min(1)).nullable().optional(),
    }),
  ),
});
