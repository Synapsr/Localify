import { useMutation, useQueryClient } from "@tanstack/react-query";
import { projectMutations } from "../lib/queries";
import type { DomainInfoDto, ProjectInfoDto } from "../types/roxy";
import { DomainCard } from "./domain-card";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card } from "./ui/card";

interface ProjectCardProps {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
  projects: ProjectInfoDto[];
  onAddDomain: (projectId: string) => void;
}

export const ProjectCard = ({
  project,
  domains,
  projects,
  onAddDomain,
}: ProjectCardProps) => {
  const queryClient = useQueryClient();
  const updateProject = useMutation(projectMutations.update(queryClient));

  async function handleToggle() {
    await updateProject.mutateAsync({
      id: project.id,
      enabled: !project.enabled,
    });
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex items-start justify-between gap-4 border-b border-[--color-border] p-5">
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="text-base font-semibold text-[--color-ink]">
              {project.name}
            </h2>
            <Badge tone={project.enabled ? "accent" : "muted"}>
              {project.enabled ? "Enabled" : "Disabled"}
            </Badge>
            <Badge tone="muted">
              {project.domainCount} domain{project.domainCount === 1 ? "" : "s"}
            </Badge>
          </div>
          <p className="text-sm text-[--color-muted]">
            Toggle the whole project to enable or disable every attached domain.
          </p>
        </div>

        <div className="flex shrink-0 items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => onAddDomain(project.id)}
          >
            Add domain
          </Button>
          <Button
            type="button"
            size="sm"
            variant={project.enabled ? "secondary" : "primary"}
            loading={updateProject.isPending}
            onClick={handleToggle}
          >
            {project.enabled ? "Disable" : "Enable"}
          </Button>
        </div>
      </div>

      <div className="space-y-3 p-5">
        {domains.length === 0 ? (
          <div className="rounded-lg border border-dashed border-[--color-border] p-4">
            <p className="text-sm text-[--color-muted]">
              No domains attached yet. Add a web app, API, or any other local
              entrypoint to this project.
            </p>
            <div className="pt-3">
              <Button
                type="button"
                size="sm"
                variant="secondary"
                onClick={() => onAddDomain(project.id)}
              >
                Add first domain
              </Button>
            </div>
          </div>
        ) : (
          domains.map((domain) => (
            <DomainCard
              key={domain.registration.pattern}
              domain={domain}
              projects={projects}
            />
          ))
        )}
      </div>
    </Card>
  );
};
