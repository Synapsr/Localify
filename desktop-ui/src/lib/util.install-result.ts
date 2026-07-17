import type { InstallResultDto } from "@/types/localify";

export function getInstallWarningMessage(result: InstallResultDto): string | null {
  const warnings = result.steps.filter((step) => step.status === "warning");

  if (warnings.length === 0) {
    return null;
  }

  return warnings
    .map((step) => (step.label ? `${step.label}: ${step.message}` : step.message))
    .join("\n");
}

export function assertInstallCompleted(result: InstallResultDto): void {
  const warning = getInstallWarningMessage(result);

  if (warning) {
    throw new Error(warning);
  }
}
