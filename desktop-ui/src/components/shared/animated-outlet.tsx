import {
  Outlet,
  RouterContextProvider,
  useMatches,
  useRouter,
} from "@tanstack/react-router";
import { motion, useIsPresent } from "motion/react";
import { forwardRef, type ComponentProps, useRef } from "react";

type FrozenStore<T> = {
  state: T;
  get: () => T;
  subscribe: (_listener: () => void) => { unsubscribe: () => void };
};

const createFrozenStore = <T,>(value: T): FrozenStore<T> => {
  return {
    state: value,
    get: () => value,
    subscribe: () => ({ unsubscribe: () => {} }),
  };
};

export const AnimatedOutlet = forwardRef<
  HTMLDivElement,
  ComponentProps<typeof motion.div>
>(function AnimatedOutlet({ style, ...props }, ref) {
  const isPresent = useIsPresent();
  const matches = useMatches();
  const router = useRouter();
  const previousMatchesRef = useRef(matches);
  const previousStateRef = useRef(router.state);
  const frozenRouterRef = useRef<typeof router | null>(null);

  let renderedRouter = router;

  if (isPresent) {
    previousMatchesRef.current = matches;
    previousStateRef.current = router.state;
    frozenRouterRef.current = null;
  } else if (!frozenRouterRef.current) {
    const patchedMatches = [
      ...matches.map((match, index) => ({
        ...(previousMatchesRef.current[index] ?? match),
        id: match.id,
      })),
      ...previousMatchesRef.current.slice(matches.length),
    ];

    const patchedState = {
      ...previousStateRef.current,
      matches: patchedMatches,
    };

    const frozenMatchStores = new Map(
      router.stores.matchStores,
    ) as typeof router.stores.matchStores;
    const frozenRouteMatchStores = new Map<
      string,
      ReturnType<typeof createFrozenStore<(typeof patchedMatches)[number]>>
    >();

    for (const match of patchedMatches) {
      const store = createFrozenStore(match);
      frozenMatchStores.set(match.id, store as never);
      frozenRouteMatchStores.set(match.routeId, store);
    }

    const frozenStores = Object.create(router.stores) as typeof router.stores;

    Object.defineProperty(frozenStores, "__store", {
      value: createFrozenStore(patchedState),
      configurable: true,
    });
    Object.defineProperty(frozenStores, "matches", {
      value: createFrozenStore(patchedMatches),
      configurable: true,
    });
    Object.defineProperty(frozenStores, "matchesId", {
      value: createFrozenStore(patchedMatches.map((match) => match.id)),
      configurable: true,
    });
    Object.defineProperty(frozenStores, "matchStores", {
      value: frozenMatchStores,
      configurable: true,
    });
    Object.defineProperty(frozenStores, "getRouteMatchStore", {
      value: (routeId: string) => {
        return frozenRouteMatchStores.get(routeId) ?? router.stores.getRouteMatchStore(routeId);
      },
      configurable: true,
    });

    const frozenRouter = Object.create(router) as typeof router;
    Object.defineProperty(frozenRouter, "stores", {
      value: frozenStores,
      configurable: true,
    });
    Object.defineProperty(frozenRouter, "state", {
      get: () => patchedState,
      configurable: true,
    });

    frozenRouterRef.current = frozenRouter;
  }

  if (frozenRouterRef.current) {
    renderedRouter = frozenRouterRef.current;
  }

  return (
    <motion.div
      ref={ref}
      style={{ ...style, pointerEvents: isPresent ? "auto" : "none" }}
      {...props}
    >
      <RouterContextProvider router={renderedRouter}>
        <Outlet />
      </RouterContextProvider>
    </motion.div>
  );
});
