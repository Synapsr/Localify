import { useEffect, useState, type RefObject } from "react";

interface ScrollState {
  scrollTop: number;
  hasScrolled: boolean;
  isSticky: boolean;
}

const INITIAL_SCROLL_STATE: ScrollState = {
  scrollTop: 0,
  hasScrolled: false,
  isSticky: false,
};

function findScrollParent(element: HTMLElement): HTMLElement | null {
  let parent: HTMLElement | null = element.parentElement;

  while (parent) {
    const { overflowY } = getComputedStyle(parent);
    if (overflowY === "auto" || overflowY === "scroll" || overflowY === "overlay") {
      return parent;
    }
    parent = parent.parentElement;
  }

  return null;
}

export const useScrollState = (ref: RefObject<HTMLElement | null>) => {
  const [state, setState] = useState(INITIAL_SCROLL_STATE);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;

    const scrollParent = findScrollParent(element);
    if (!scrollParent) return;

    const update = () => {
      const scrollTop = scrollParent.scrollTop;
      const elementRect = element.getBoundingClientRect();
      const parentRect = scrollParent.getBoundingClientRect();
      const stickyTop = Number.parseFloat(getComputedStyle(element).top) || 0;
      const parentPaddingTop = Number.parseFloat(getComputedStyle(scrollParent).paddingTop) || 0;
      const stickyThreshold = parentRect.top + parentPaddingTop + stickyTop;
      const nextState = {
        scrollTop,
        hasScrolled: scrollTop > 0,
        isSticky: elementRect.top <= stickyThreshold + 0.5,
      };

      setState((currentState) => {
        if (
          currentState.scrollTop === nextState.scrollTop &&
          currentState.hasScrolled === nextState.hasScrolled &&
          currentState.isSticky === nextState.isSticky
        ) {
          return currentState;
        }

        return nextState;
      });
    };

    update();
    scrollParent.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);

    return () => {
      scrollParent.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [ref]);

  return state;
};
