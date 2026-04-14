import { useState, type FormEvent } from "react";

import {
  useAvailablePort,
  useCreateProject,
  useRegisterDomain,
} from "../lib/queries";
import { Button } from "./ui/button";
import { Input, Label } from "./ui/input";

interface CreateProjectFormProps {
  suffix: string;
  onClose: () => void;
}

export const CreateProjectForm = ({
  suffix,
  onClose,
}: CreateProjectFormProps) => {
  const availablePort = useAvailablePort();
  const createProject = useCreateProject();
  const registerDomain = useRegisterDomain();

  const [name, setName] = useState("");
  const [appDomain, setAppDomain] = useState("");
  const [appTarget, setAppTarget] = useState("");
  const [includeApi, setIncludeApi] = useState(false);
  const [apiDomain, setApiDomain] = useState("api");
  const [apiTarget, setApiTarget] = useState("");
  const [error, setError] = useState<string | null>(null);

  const normalizedSuffix = suffix.startsWith(".")
    ? suffix
    : `.${suffix || "localify"}`;
  const suggestedDomain = slugify(name) || "my-app";
  const appPreview = normalizeDomain(appDomain || suggestedDomain, normalizedSuffix);
  const apiPreview = normalizeDomain(apiDomain || "api", normalizedSuffix);

  const hasAppDraft = appDomain.trim().length > 0 || appTarget.trim().length > 0;
  const appReady = appDomain.trim().length > 0 && appTarget.trim().length > 0;
  const apiReady = apiDomain.trim().length > 0 && apiTarget.trim().length > 0;
  const submitDisabled =
    !name.trim() || (hasAppDraft && !appReady) || (includeApi && !apiReady);

  async function handleGeneratePort(target: "app" | "api") {
    setError(null);

    try {
      const { port } = await availablePort.mutateAsync();
      if (target === "app") {
        setAppTarget(String(port));
        return;
      }

      setApiTarget(String(port));
    } catch (portError) {
      setError(portError instanceof Error ? portError.message : String(portError));
    }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (submitDisabled) {
      return;
    }

    setError(null);

    let createdProjectName = "";

    try {
      const project = await createProject.mutateAsync({ name });
      createdProjectName = project.name;

      if (appReady) {
        await registerDomain.mutateAsync({
          domain: normalizeDomain(appDomain, normalizedSuffix),
          wildcard: false,
          routes: [appTarget],
          projectId: project.id,
        });
      }

      if (includeApi) {
        await registerDomain.mutateAsync({
          domain: normalizeDomain(apiDomain, normalizedSuffix),
          wildcard: false,
          routes: [apiTarget],
          projectId: project.id,
        });
      }

      onClose();
    } catch (submitError) {
      const message =
        submitError instanceof Error ? submitError.message : String(submitError);
      setError(
        createdProjectName
          ? `${createdProjectName} was created, but the initial domain setup failed: ${message}`
          : message,
      );
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="space-y-1">
          <h2 className="text-sm font-semibold text-[--color-ink]">New project</h2>
          <p className="text-xs text-[--color-muted]">
            Create an empty project, or set up your first local domains immediately.
          </p>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="text-xs text-[--color-muted] hover:text-[--color-ink]"
        >
          Cancel
        </button>
      </div>

      <div className="space-y-1.5">
        <Label htmlFor="project-name">Project name</Label>
        <Input
          id="project-name"
          placeholder="Test 1"
          value={name}
          onChange={(event) => setName(event.target.value)}
          autoFocus
        />
      </div>

      <div className="space-y-3 rounded-xl border border-[--color-border] p-4">
        <div className="space-y-1">
          <h3 className="text-sm font-medium text-[--color-ink]">Primary domain</h3>
          <p className="text-xs text-[--color-muted]">
            Optional. Use this to create the first app domain while setting up the project.
          </p>
        </div>

        <div className="grid gap-4 sm:grid-cols-2">
          <div className="space-y-1.5">
            <Label htmlFor="app-domain">Domain</Label>
            <Input
              id="app-domain"
              placeholder={suggestedDomain}
              value={appDomain}
              onChange={(event) => setAppDomain(event.target.value)}
            />
            <p className="font-mono text-xs text-[--color-muted]">{appPreview}</p>
          </div>

          <div className="space-y-1.5">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="app-target">Target</Label>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                loading={availablePort.isPending}
                onClick={() => handleGeneratePort("app")}
              >
                Free port
              </Button>
            </div>
            <Input
              id="app-target"
              placeholder="3000"
              value={appTarget}
              onChange={(event) => setAppTarget(event.target.value)}
            />
          </div>
        </div>
      </div>

      <div className="space-y-3 rounded-xl border border-[--color-border] p-4">
        <label className="flex items-center gap-2 text-sm text-[--color-muted]">
          <input
            type="checkbox"
            checked={includeApi}
            onChange={(event) => setIncludeApi(event.target.checked)}
            className="h-4 w-4 rounded border-[--color-border] accent-[--color-ink]"
          />
          Also create an API domain in this project
        </label>

        {includeApi && (
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-1.5">
              <Label htmlFor="api-domain">API domain</Label>
              <Input
                id="api-domain"
                placeholder="api"
                value={apiDomain}
                onChange={(event) => setApiDomain(event.target.value)}
              />
              <p className="font-mono text-xs text-[--color-muted]">{apiPreview}</p>
            </div>

            <div className="space-y-1.5">
              <div className="flex items-center justify-between gap-3">
                <Label htmlFor="api-target">API target</Label>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  loading={availablePort.isPending}
                  onClick={() => handleGeneratePort("api")}
                >
                  Free port
                </Button>
              </div>
              <Input
                id="api-target"
                placeholder="3001"
                value={apiTarget}
                onChange={(event) => setApiTarget(event.target.value)}
              />
            </div>
          </div>
        )}
      </div>

      {error && <p className="text-xs text-[--color-danger]">{error}</p>}

      <div className="flex items-center justify-end gap-2 border-t border-[--color-border] pt-4">
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button
          type="submit"
          variant="primary"
          loading={createProject.isPending || registerDomain.isPending}
          disabled={submitDisabled}
        >
          Create project
        </Button>
      </div>
    </form>
  );
};

function normalizeDomain(value: string, suffix: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    return `example${suffix}`;
  }

  return trimmed.endsWith(suffix) ? trimmed : `${trimmed}${suffix}`;
}

function slugify(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
