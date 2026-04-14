import { useState } from "react";

import { AddDomainForm } from "../components/add-domain-form";
import { CreateProjectForm } from "../components/create-project-form";
import { DomainCard } from "../components/domain-card";
import { ProjectCard } from "../components/project-card";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { EmptyState } from "../components/ui/empty-state";
import { useBootstrap } from "../lib/queries";
import {
  buildProjectGroups,
  listUnassignedDomains,
} from "../lib/util.project-groups";

export function DomainsScreen() {
  const { data } = useBootstrap();
  const [addingDomainForProject, setAddingDomainForProject] = useState<
    string | null | undefined
  >(undefined);
  const [creatingProject, setCreatingProject] = useState(false);

  const domains = data?.domains ?? [];
  const projects = data?.projects ?? [];
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const projectGroups = buildProjectGroups(projects, domains);
  const unassignedDomains = listUnassignedDomains(domains);
  const hasContent = projectGroups.length > 0 || unassignedDomains.length > 0;

  function openDomainForm(projectId: string | null = null) {
    setAddingDomainForProject(projectId);
    setCreatingProject(false);
  }

  function openProjectForm() {
    setCreatingProject(true);
    setAddingDomainForProject(undefined);
  }

  function closeForms() {
    setAddingDomainForProject(undefined);
    setCreatingProject(false);
  }

  return (
    <section className="space-y-6">
      <div className="flex items-end justify-between">
        <div className="space-y-1">
          <h1 className="text-xl font-semibold tracking-tight">Domains</h1>
          <p className="text-sm text-[--color-muted]">
            Register local domains and route requests to your apps.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {!creatingProject && (
            <Button variant="secondary" onClick={openProjectForm}>
              New project
            </Button>
          )}
          {addingDomainForProject === undefined && (
            <Button variant="primary" onClick={() => openDomainForm()}>
              New domain
            </Button>
          )}
        </div>
      </div>

      {creatingProject && (
        <Card className="p-5">
          <CreateProjectForm suffix={suffix} onClose={closeForms} />
        </Card>
      )}

      {addingDomainForProject !== undefined && (
        <Card className="p-5">
          <AddDomainForm
            suffix={suffix}
            projects={projects}
            defaultProjectId={addingDomainForProject}
            onClose={closeForms}
          />
        </Card>
      )}

      {!hasContent &&
        !creatingProject &&
        addingDomainForProject === undefined && (
        <EmptyState
          title="No projects or domains yet"
          description={`Create a project to group related ${suffix || ".localify"} domains, or add a single domain without a project.`}
          action={
            <div className="flex items-center gap-2">
              <Button variant="secondary" onClick={openProjectForm}>
                Add project
              </Button>
              <Button variant="primary" onClick={() => openDomainForm()}>
                Add domain
              </Button>
            </div>
          }
        />
      )}

      {projectGroups.length > 0 && (
        <div className="space-y-4">
          {projectGroups.map((group) => (
            <ProjectCard
              key={group.project.id}
              project={group.project}
              domains={group.domains}
              projects={projects}
              onAddDomain={openDomainForm}
            />
          ))}
        </div>
      )}

      {unassignedDomains.length > 0 && (
        <div className="space-y-3">
          <div className="space-y-1">
            <h2 className="text-sm font-semibold text-[--color-ink]">
              Unassigned domains
            </h2>
            <p className="text-sm text-[--color-muted]">
              Domains not attached to any project still work independently.
            </p>
          </div>
          <ul className="space-y-3">
            {unassignedDomains.map((domain) => (
              <li key={domain.registration.pattern}>
                <DomainCard domain={domain} projects={projects} />
              </li>
            ))}
          </ul>
        </div>
      )}

      {projects.length === 0 && domains.length > 0 && !creatingProject && (
        <Card className="p-5">
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <h2 className="text-sm font-semibold text-[--color-ink]">
                Group existing domains into projects
              </h2>
              <p className="text-sm text-[--color-muted]">
                Create a project, then attach future domains to it directly from
                the create form.
              </p>
            </div>
            <Button variant="secondary" onClick={openProjectForm}>
              Create first project
            </Button>
          </div>
        </Card>
      )}

      {domains.length === 0 && projects.length > 0 && !creatingProject && (
        <Card className="p-5">
          <div className="flex items-center justify-between gap-4">
            <div className="space-y-1">
              <h2 className="text-sm font-semibold text-[--color-ink]">
                Projects are ready
              </h2>
              <p className="text-sm text-[--color-muted]">
                Start attaching domains like `web{suffix}` or `api{suffix}` to
                each project.
              </p>
            </div>
            <Button
              variant="primary"
              onClick={() => openDomainForm(projects[0]?.id ?? null)}
            >
              Add first domain
            </Button>
          </div>
        </Card>
      )}
    </section>
  );
}
