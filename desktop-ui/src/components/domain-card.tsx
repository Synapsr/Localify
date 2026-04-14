import { useState, type FormEvent } from "react";
import { Card } from "./ui/card";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import {
  useAddRoute,
  useAssignDomainProject,
  useRemoveRoute,
  useUnregisterDomain,
} from "../lib/queries";
import type { DomainInfoDto, ProjectInfoDto, RouteDto } from "../types/roxy";

interface DomainCardProps {
  domain: DomainInfoDto;
  projects: ProjectInfoDto[];
}

export function DomainCard({ domain, projects }: DomainCardProps) {
  const { registration } = domain;
  const addRoute = useAddRoute();
  const assignProject = useAssignDomainProject();
  const removeRoute = useRemoveRoute();
  const unregister = useUnregisterDomain();

  const [addingRoute, setAddingRoute] = useState(false);
  const [editingProject, setEditingProject] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [projectError, setProjectError] = useState<string | null>(null);
  const [newPath, setNewPath] = useState("/");
  const [newTarget, setNewTarget] = useState("");
  const [selectedProjectId, setSelectedProjectId] = useState(domain.projectId ?? "");

  async function handleAddRoute(e: FormEvent) {
    e.preventDefault();
    if (!newPath || !newTarget) return;
    await addRoute.mutateAsync({
      domain: registration.domain,
      wildcard: registration.wildcard,
      path: newPath,
      target: newTarget,
    });
    setNewPath("/");
    setNewTarget("");
    setAddingRoute(false);
  }

  async function handleRemoveRoute(route: RouteDto) {
    await removeRoute.mutateAsync({
      domain: registration.domain,
      wildcard: registration.wildcard,
      path: route.path,
    });
  }

  async function handleDelete() {
    setDeleteError(null);
    try {
      await unregister.mutateAsync({
        domain: registration.domain,
        wildcard: registration.wildcard,
      });
      setConfirmingDelete(false);
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleAssignProject() {
    setProjectError(null);
    try {
      await assignProject.mutateAsync({
        domain: registration.domain,
        wildcard: registration.wildcard,
        projectId: selectedProjectId || null,
      });
      setEditingProject(false);
    } catch (error) {
      setProjectError(error instanceof Error ? error.message : String(error));
    }
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex items-start justify-between gap-4 p-5">
        <div className="min-w-0 flex-1 space-y-2">
          <div className="flex items-center gap-2">
            <h3 className="truncate font-mono text-sm font-semibold text-[--color-ink]">
              {registration.pattern}
            </h3>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            {registration.httpsEnabled && <Badge tone="accent">HTTPS</Badge>}
            {registration.wildcard && <Badge tone="muted">Wildcard</Badge>}
            {domain.projectName && <Badge tone="muted">{domain.projectName}</Badge>}
            {!domain.projectEnabled && <Badge tone="muted">Project disabled</Badge>}
            {!domain.active && <Badge tone="muted">Inactive</Badge>}
            {registration.source !== "config" && (
              <Badge tone="muted">{registration.source}</Badge>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          {projects.length > 0 && !confirmingDelete && (
            <button
              type="button"
              onClick={() => {
                setSelectedProjectId(domain.projectId ?? "");
                setProjectError(null);
                setEditingProject((value) => !value);
              }}
              className="text-xs text-[--color-muted] hover:text-[--color-ink]"
            >
              {editingProject ? "Close" : "Project"}
            </button>
          )}
          {confirmingDelete ? (
            <div className="flex items-center gap-2">
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => {
                  setConfirmingDelete(false);
                  setDeleteError(null);
                }}
                disabled={unregister.isPending}
              >
                Cancel
              </Button>
              <Button
                type="button"
                size="sm"
                variant="danger"
                loading={unregister.isPending}
                onClick={handleDelete}
              >
                Confirm
              </Button>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => {
                setConfirmingDelete(true);
                setDeleteError(null);
              }}
              disabled={unregister.isPending}
              className="text-xs text-[--color-muted] hover:text-[--color-danger] disabled:opacity-50"
            >
              Delete
            </button>
          )}
        </div>
      </div>

      {editingProject && (
        <div className="border-t border-[--color-border] bg-[--color-bg]/30 px-5 py-3">
          <div className="flex items-center gap-2">
            <select
              value={selectedProjectId}
              onChange={(event) => setSelectedProjectId(event.target.value)}
              className="h-8 min-w-44 rounded-md border border-[--color-border] bg-[--color-surface] px-3 text-xs text-[--color-ink] focus:outline-none focus:border-[--color-border-strong] focus:ring-2 focus:ring-[--color-ink]/10"
            >
              <option value="">No project</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
            <Button
              type="button"
              size="sm"
              variant="secondary"
              loading={assignProject.isPending}
              onClick={handleAssignProject}
            >
              Save
            </Button>
          </div>
          {projectError && (
            <p className="pt-2 text-xs text-[--color-danger]">{projectError}</p>
          )}
        </div>
      )}

      {confirmingDelete && (
        <div className="border-t border-[--color-danger]/20 bg-[--color-danger]/5 px-5 py-3">
          <p className="text-xs text-[--color-muted]">
            Delete <span className="font-mono">{registration.pattern}</span> and
            remove its {registration.routes.length} route
            {registration.routes.length > 1 ? "s" : ""}.
          </p>
          {deleteError && (
            <p className="pt-2 text-xs text-[--color-danger]">{deleteError}</p>
          )}
        </div>
      )}

      <div className="border-t border-[--color-border] bg-[--color-bg]/40">
        {registration.routes.length === 0 ? (
          <p className="px-5 py-4 text-xs text-[--color-muted]">
            No routes yet.
          </p>
        ) : (
          <ul className="divide-y divide-[--color-border]">
            {registration.routes.map((route) => (
              <li
                key={route.path}
                className="flex items-center gap-3 px-5 py-2.5"
              >
                <span className="font-mono text-xs text-[--color-muted]">
                  {route.path}
                </span>
                <span className="text-[--color-muted]">→</span>
                <span className="flex-1 truncate font-mono text-xs text-[--color-ink]">
                  {route.target}
                </span>
                <span className="text-xs text-[--color-muted]">
                  {route.targetKind}
                </span>
                <button
                  type="button"
                  onClick={() => handleRemoveRoute(route)}
                  className="text-xs text-[--color-muted] hover:text-[--color-danger]"
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}

        {addingRoute ? (
          <form
            onSubmit={handleAddRoute}
            className="flex items-center gap-2 border-t border-[--color-border] px-5 py-3"
          >
            <Input
              placeholder="/"
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              className="h-8 w-24 text-xs"
            />
            <span className="text-[--color-muted]">→</span>
            <Input
              placeholder="3000 or ./dist"
              value={newTarget}
              onChange={(e) => setNewTarget(e.target.value)}
              className="h-8 flex-1 text-xs"
              autoFocus
            />
            <Button
              type="submit"
              size="sm"
              variant="primary"
              loading={addRoute.isPending}
              disabled={!newTarget}
            >
              Add
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => setAddingRoute(false)}
            >
              Cancel
            </Button>
          </form>
        ) : (
          <button
            type="button"
            onClick={() => setAddingRoute(true)}
            className="w-full border-t border-[--color-border] px-5 py-2.5 text-left text-xs text-[--color-muted] hover:text-[--color-ink]"
          >
            + Add route
          </button>
        )}
      </div>
    </Card>
  );
}
