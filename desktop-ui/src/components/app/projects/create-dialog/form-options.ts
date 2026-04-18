import { formOptions } from "@tanstack/react-form";

import {
  type CreateProjectInput,
  emptyDomainEntry,
} from "@/lib/validators/validator.create-project";
import { type InitProjectFromFolderInput } from "@/lib/validators/validator.init-project-from-folder";

export const manualFormOpts = formOptions({
  defaultValues: {
    name: "",
    domains: [emptyDomainEntry()],
  } as CreateProjectInput,
});

export const folderFormOpts = formOptions({
  defaultValues: {
    rootPath: "",
    name: "",
    services: [],
  } as InitProjectFromFolderInput,
});
