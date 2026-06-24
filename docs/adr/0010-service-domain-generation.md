# Service Domain Generation

Localify generates different default local domains for services depending on project shape. A single-service project gets the concise root-style local domain such as `project.localify`, while a multi-service project gets service-scoped domains such as `web.project.localify`, `api.project.localify`, and `admin.project.localify`. This keeps the simplest projects pleasant while avoiding the false implication that a monorepo has one canonical service.
