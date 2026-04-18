import { useMatches, useRouterState } from "@tanstack/react-router";
import { AnimatePresence, useReducedMotion, type Transition, type Variants } from "motion/react";
import { useRef } from "react";

import { ScrollProvider, useScrollTargetRef } from "@/hooks/use-scroll";

import { AnimatedOutlet } from "./animated-outlet";

const ROUTE_ORDER = ["/projects", "/logs", "/settings"] as const;
type TransitionDirection = -1 | 1;

function getRouteIndex(pathname: string): number {
  return ROUTE_ORDER.findIndex((route) => pathname.startsWith(route));
}

function getTransitionDirection(
  previousPathname: string,
  pathname: string,
  previousHistoryIndex: number,
  historyIndex: number,
): TransitionDirection {
  const previousRouteIndex = getRouteIndex(previousPathname);
  const routeIndex = getRouteIndex(pathname);

  if (previousRouteIndex !== -1 && routeIndex !== -1 && previousRouteIndex !== routeIndex) {
    return routeIndex > previousRouteIndex ? 1 : -1;
  }

  if (historyIndex !== previousHistoryIndex) {
    return historyIndex > previousHistoryIndex ? 1 : -1;
  }

  return 1;
}

export function PageTransition() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const historyIndex = useRouterState({
    select: (state) => state.location.state.__TSR_index,
  });
  const routeKey = useMatches({
    select: (matches) => matches[1]?.id ?? matches.at(-1)?.id ?? pathname,
  });
  const previousPathnameRef = useRef(pathname);
  const previousHistoryIndexRef = useRef(historyIndex);
  const directionRef = useRef<TransitionDirection>(1);
  const reduceMotion = useReducedMotion();

  if (previousPathnameRef.current !== pathname) {
    directionRef.current = getTransitionDirection(
      previousPathnameRef.current,
      pathname,
      previousHistoryIndexRef.current,
      historyIndex,
    );
    previousPathnameRef.current = pathname;
    previousHistoryIndexRef.current = historyIndex;
  }

  const direction = directionRef.current;
  const variants: Variants = reduceMotion
    ? {
        initial: { opacity: 0 },
        animate: { opacity: 1 },
        exit: { opacity: 0 },
      }
    : {
        initial: (currentDirection: TransitionDirection) => ({
          x: `${currentDirection * 100}%`,
          scale: 0.892,
          filter: "blur(6px)",
        }),
        animate: {
          x: "0%",
          scale: 1,
          filter: "blur(0px)",
        },
        exit: (currentDirection: TransitionDirection) => ({
          x: `${currentDirection * -100}%`,
          scale: 0.892,
          filter: "blur(6px)",
        }),
      };

  const transition: Transition = reduceMotion
    ? { duration: 0.14, ease: "easeOut" }
    : {
        type: "tween",
        duration: 0.34,
        ease: [0.22, 1, 0.36, 1],
      };

  return (
    <ScrollProvider>
      <div className="relative h-full w-full overflow-hidden ">
        <AnimatePresence initial={false} custom={direction}>
          {/* header will be sticky here */}
          {/* header portal */}
          <ScrollableOutlet
            key={routeKey}
            direction={direction}
            variants={variants}
            transition={transition}
          />
        </AnimatePresence>
      </div>
    </ScrollProvider>
  );
}

function ScrollableOutlet({
  direction,
  variants,
  transition,
}: {
  direction: TransitionDirection;
  variants: Variants;
  transition: Transition;
}) {
  const setScrollTarget = useScrollTargetRef();
  return (
    <AnimatedOutlet
      ref={setScrollTarget}
      custom={direction}
      initial="initial"
      animate="animate"
      exit="exit"
      variants={variants}
      transition={transition}
      className="absolute inset-0 origin-left will-change-transform"
      scrollClassName="h-full p-6 pt-0"
      style={{
        willChange: "transform, filter",
        transformOrigin: direction === 1 ? "right center" : "left center",
      }}
    />
  );
}
