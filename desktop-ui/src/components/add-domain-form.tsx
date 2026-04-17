import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Label } from "./ui/label";
import { domainMutations, utilityMutations } from "../lib/queries";
import type { ProjectInfoDto } from "../types/roxy";

interface AddDomainFormProps {
  suffix: string;
  projects: ProjectInfoDto[];
  defaultProjectId?: string | null;
  onClose: () => void;
}

export function AddDomainForm({
  suffix,
  projects,
  defaultProjectId = null,
  onClose,
}: AddDomainFormProps) {
  const queryClient = useQueryClient();
  const registerDomain = useMutation(domainMutations.register(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [wildcard, setWildcard] = useState(false);
  const [projectId, setProjectId] = useState<string>(defaultProjectId ?? "");
  const [error, setError] = useState<string | null>(null);
  const [targetError, setTargetError] = useState<string | null>(null);

  const normalizedSuffix = suffix.startsWith(".") ? suffix : `.${suffix || "localify"}`;
  const previewDomain = name
    ? name.endsWith(normalizedSuffix)
      ? name
      : `${name}${normalizedSuffix}`
    : `example${normalizedSuffix}`;

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    if (!name || !target) return;
    setError(null);

    const domain = name.endsWith(normalizedSuffix) ? name : `${name}${normalizedSuffix}`;

    try {
      await registerDomain.mutateAsync({
        domain,
        wildcard,
        routes: [target],
        projectId: projectId || null,
      });
      setName("");
      setTarget("");
      setWildcard(false);
      setProjectId(defaultProjectId ?? "");
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function handleGeneratePort() {
    setTargetError(null);

    try {
      const { port } = await availablePort.mutateAsync();
      setTarget(String(port));
    } catch (err) {
      setTargetError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-5">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold text-[--color-ink]">New domain</h2>
        <button
          type="button"
          onClick={onClose}
          className="text-xs text-[--color-muted] hover:text-[--color-ink]"
        >
          Cancel
        </button>
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <Label htmlFor="domain-name">Domain</Label>
          <div className="flex items-center gap-1.5">
            <Input
              id="domain-name"
              placeholder="my-app"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoFocus
            />
          </div>
          <p className="font-mono text-xs text-[--color-muted]">{previewDomain}</p>
        </div>

        <div className="space-y-1.5">
          <div className="flex items-center justify-between gap-3">
            <Label htmlFor="target">Target</Label>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              loading={availablePort.isPending}
              onClick={handleGeneratePort}
            >
              Free port
            </Button>
          </div>
          <Input
            id="target"
            placeholder="3000 or ./public"
            value={target}
            onChange={(e) => {
              setTarget(e.target.value);
              setTargetError(null);
            }}
          />
          {targetError ? (
            <p className="text-xs text-[--color-danger]">{targetError}</p>
          ) : (
            <p className="text-xs text-[--color-muted]">
              Port number, URL, or path to a directory. Use Free port to fill an unused local port.
            </p>
          )}
        </div>
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="project">Project</Label>
        <select
          id="project"
          value={projectId}
          onChange={(event) => setProjectId(event.target.value)}
          className="h-9 w-full rounded-md border border-[--color-border] bg-[--color-surface] px-3 text-sm text-[--color-ink] focus:outline-none focus:border-[--color-border-strong] focus:ring-2 focus:ring-[--color-ink]/10"
        >
          <option value="">No project</option>
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.name}
            </option>
          ))}
        </select>
        <p className="text-xs text-[--color-muted]">
          Optional. Attach this domain to a project so it can be enabled or disabled with the rest
          of the stack.
        </p>
      </div>

      <label className="flex items-center gap-2 text-sm text-[--color-muted]">
        <input
          type="checkbox"
          checked={wildcard}
          onChange={(e) => setWildcard(e.target.checked)}
          className="h-4 w-4 rounded border-[--color-border] accent-[--color-ink]"
        />
        Match all subdomains (wildcard)
      </label>

      {error && <p className="text-xs text-[--color-danger]">{error}</p>}

      <div className="flex items-center justify-end gap-2 border-t border-[--color-border] pt-4">
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button
          type="submit"
          variant="primary"
          loading={registerDomain.isPending}
          disabled={!name || !target}
        >
          Create domain
        </Button>
      </div>
    </form>
  );
}
