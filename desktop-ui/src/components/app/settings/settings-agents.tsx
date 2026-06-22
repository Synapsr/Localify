"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";

import { CheckIcon, ChevronDownIcon, InfoCircleIcon, SparklesIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { configMutations, systemMutations, systemQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { AppConfigDto } from "@/types/roxy";

import { SettingsCard } from "./settings-card";

type SettingsAgentsProps = {
  config: AppConfigDto;
};

export const SettingsAgents = ({ config }: SettingsAgentsProps) => {
  const queryClient = useQueryClient();
  const { data: skillStatus } = useQuery(systemQueries.agentSkillStatus());
  const updateConfig = useMutation(configMutations.update(queryClient));
  const installSkill = useMutation(systemMutations.installAgentSkill(queryClient));
  const [open, setOpen] = useState(false);

  const detectedAgentOptions =
    skillStatus?.detectedAgents.filter((agent) => agent.detected) ?? [];
  const detectedAgents = detectedAgentOptions.map((agent) => agent.label);
  const missingAgentOptions = detectedAgentOptions.filter((agent) => !agent.installed);
  const allDetectedAgentsInstalled =
    detectedAgentOptions.length > 0 && missingAgentOptions.length === 0;
  const detectedAgentsLabel =
    detectedAgents.length > 0 ? detectedAgents.join(", ") : "No supported agent detected";
  const missingAgentsLabel =
    missingAgentOptions.length > 0
      ? missingAgentOptions.map((agent) => agent.label).join(", ")
      : detectedAgentsLabel;
  const skillReadiness = (() => {
    if (!skillStatus) {
      return {
        tone: "loading" as const,
        title: "Checking agent setup",
        description: "Localify is looking for compatible coding agents on this Mac.",
      };
    }
    if (detectedAgents.length === 0) {
      return {
        tone: "warning" as const,
        title: "No compatible agent found yet",
        description: "Install Codex, Claude Code, or Cursor, then come back to connect Localify.",
      };
    }
    if (!skillStatus.npxAvailable) {
      return {
        tone: "warning" as const,
        title: "Agent setup needs Node.js",
        description: "Install Node.js so Localify can connect the skill to your coding agents.",
      };
    }
    if (allDetectedAgentsInstalled) {
      return {
        tone: "ready" as const,
        title: "Agents connected",
        description: "Localify is already available to compatible coding agents on this Mac.",
      };
    }
    return {
      tone: "ready" as const,
      title: "Ready to connect",
      description: `Localify can guide ${missingAgentsLabel} when they create or update projects.`,
    };
  })();
  const showInstallPrompt = !allDetectedAgentsInstalled;
  const isInstallingSkill = installSkill.isPending;
  const summary = [
    config.agent.installSkill ? "Agent guidance on" : "Agent guidance off",
    config.agent.promoteDevScript ? "Localify-first dev" : "Manual dev scripts",
    allDetectedAgentsInstalled
      ? "Agents connected"
      : detectedAgents.length > 0
        ? detectedAgents.join(", ")
        : "No agent detected",
  ].join(" · ");

  const updateAgentConfig = async (agent: AppConfigDto["agent"]) => {
    try {
      await updateConfig.mutateAsync({ agent });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error("Could not update agent settings", { description: message });
    }
  };

  const handleInstallSkill = async () => {
    try {
      const result = await installSkill.mutateAsync();
      queryClient.setQueryData(systemQueries.agentSkillStatus().queryKey, result.status);
      const installedAgentLabels = result.status.detectedAgents
        .filter((agent) => result.installedAgents.includes(agent.id))
        .map((agent) => agent.label);
      toast.success("Localify skill installed", {
        description: installedAgentLabels.join(", "),
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error("Skill installation failed", { description: message });
    }
  };

  const panelId = "settings-ai-agents-panel";

  return (
    <SettingsCard
      icon={<SparklesIcon />}
      title="AI agents"
      description={summary}
      action={
        <Button
          size="icon-lg"
          className="rounded-full"
          variant="outline"
          aria-expanded={open}
          aria-controls={panelId}
          aria-label={open ? "Collapse AI agent settings" : "Expand AI agent settings"}
          onClick={() => setOpen((current) => !current)}
        >
          <ChevronDownIcon
            className={cn(
              "size-4 transition-transform duration-300 ease-out",
              open && "rotate-180",
            )}
          />
        </Button>
      }
    >
      <div
        id={panelId}
        role="region"
        className={cn(
          "grid transition-[grid-template-rows] duration-300 ease-out",
          open ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          <div className="mt-5 space-y-4 border-t border-border/40 pt-5">
            <SettingToggle
              label="Set up agents automatically"
              description="Give compatible coding agents Localify's project setup instructions."
              checked={config.agent.installSkill}
              disabled={updateConfig.isPending}
              onChange={(installSkillEnabled) =>
                void updateAgentConfig({
                  ...config.agent,
                  installSkill: installSkillEnabled,
                })
              }
            />

            <SettingToggle
              label="Prefer Localify when running dev"
              description="New project setup keeps Localify ready as the default local dev flow."
              checked={config.agent.promoteDevScript}
              disabled={updateConfig.isPending}
              onChange={(promoteDevScript) =>
                void updateAgentConfig({
                  ...config.agent,
                  promoteDevScript,
                })
              }
            />

            {showInstallPrompt ? (
              <div className="flex items-start gap-3 rounded-xl border border-border/60 bg-background/70 p-3">
                <span
                  aria-hidden
                  className={cn(
                    "mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg ring-1 [&>svg]:size-4",
                    skillReadiness.tone === "ready" &&
                      "bg-primary/10 text-primary ring-primary/20",
                    skillReadiness.tone === "warning" &&
                      "bg-amber-500/10 text-amber-700 ring-amber-500/20 dark:text-amber-300",
                    skillReadiness.tone === "loading" &&
                      "bg-muted text-muted-foreground ring-border/60",
                  )}
                >
                  {skillReadiness.tone === "ready" ? <CheckIcon /> : <InfoCircleIcon />}
                </span>
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="text-sm font-medium text-foreground">{skillReadiness.title}</div>
                  <div className="text-xs leading-normal text-muted-foreground">
                    {skillReadiness.description}
                  </div>
                  {detectedAgents.length > 0 ? (
                    <div className="flex flex-wrap gap-1.5 pt-1">
                      {detectedAgentOptions.map((agent) => (
                        <span
                          key={agent.id}
                          className={cn(
                            "rounded-full px-2 py-0.5 text-[11px] font-medium",
                            agent.installed
                              ? "bg-primary/10 text-primary"
                              : "bg-muted text-muted-foreground",
                          )}
                        >
                          {agent.label}
                        </span>
                      ))}
                    </div>
                  ) : null}
                </div>
              </div>
            ) : null}

            {showInstallPrompt ? (
              <div className="flex justify-end">
                <Button
                  variant="secondary"
                  disabled={
                    isInstallingSkill ||
                    !config.agent.installSkill ||
                    detectedAgents.length === 0 ||
                    !skillStatus?.npxAvailable
                  }
                  loading={isInstallingSkill}
                  onClick={() => void handleInstallSkill()}
                >
                  {isInstallingSkill ? "Installing..." : "Install skill"}
                </Button>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </SettingsCard>
  );
};

type SettingToggleProps = {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
};

const SettingToggle = ({
  label,
  description,
  checked,
  disabled,
  onChange,
}: SettingToggleProps) => (
  <div className="flex items-center gap-3">
    <div className="min-w-0 flex-1">
      <div className="text-sm font-medium text-foreground">{label}</div>
      <div className="text-xs leading-normal text-muted-foreground">{description}</div>
    </div>
    <Switch
      aria-label={label}
      checked={checked}
      disabled={disabled}
      onCheckedChange={onChange}
    />
  </div>
);
