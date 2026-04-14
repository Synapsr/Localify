import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  redirect,
} from "@tanstack/react-router";

import { AppLayout } from "./components/app-layout";
import { DomainsScreen } from "./screens/domains";
import { LogsScreen } from "./screens/logs";
import { SettingsScreen } from "./screens/settings";

const rootRoute = createRootRoute({
  component: () => <Outlet />,
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  beforeLoad: () => {
    throw redirect({ to: "/domains" });
  },
});

const layoutRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "layout",
  component: AppLayout,
});

const domainsRoute = createRoute({
  getParentRoute: () => layoutRoute,
  path: "/domains",
  component: DomainsScreen,
});

const logsRoute = createRoute({
  getParentRoute: () => layoutRoute,
  path: "/logs",
  component: LogsScreen,
});

const settingsRoute = createRoute({
  getParentRoute: () => layoutRoute,
  path: "/settings",
  component: SettingsScreen,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  layoutRoute.addChildren([domainsRoute, logsRoute, settingsRoute]),
]);

export const router = createRouter({
  routeTree,
  history: createMemoryHistory({ initialEntries: ["/"] }),
  defaultPreload: "intent",
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
