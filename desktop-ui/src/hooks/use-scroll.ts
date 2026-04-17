import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";

type ScrollDirection = "up" | "down" | null;

type ScrollContextValue = {
  element: HTMLElement | null;
  setElement: Dispatch<SetStateAction<HTMLElement | null>>;
};

const ScrollContext = createContext<ScrollContextValue | null>(null);

export function ScrollProvider({ children }: { children: ReactNode }) {
  const [element, setElement] = useState<HTMLElement | null>(null);
  const value = useMemo(() => ({ element, setElement }), [element]);
  return createElement(ScrollContext.Provider, { value }, children);
}

export function useScrollTargetRef() {
  const ctx = useContext(ScrollContext);
  if (!ctx) {
    throw new Error("useScrollTargetRef must be used within <ScrollProvider>");
  }

  const nodeRef = useRef<HTMLElement | null>(null);

  return useCallback(
    (node: HTMLElement | null) => {
      if (node) {
        nodeRef.current = node;
        ctx.setElement(node);
        return;
      }

      ctx.setElement((currentElement) => {
        return currentElement === nodeRef.current ? null : currentElement;
      });
      nodeRef.current = null;
    },
    [ctx],
  );
}

export const useScroll = (threshold = 0) => {
  const ctx = useContext(ScrollContext);
  if (!ctx) {
    throw new Error("useScroll must be used within <ScrollProvider>");
  }
  const { element } = ctx;
  const [scrolled, setScrolled] = useState(false);
  const [direction, setDirection] = useState<ScrollDirection>(null);
  const lastY = useRef(0);

  useEffect(() => {
    if (!element) return;

    const onScroll = () => {
      const y = element.scrollTop;
      setScrolled(y > threshold);
      if (y !== lastY.current) {
        setDirection(y > lastY.current ? "down" : "up");
        lastY.current = y;
      }
    };

    onScroll();
    element.addEventListener("scroll", onScroll, { passive: true });
    return () => element.removeEventListener("scroll", onScroll);
  }, [element, threshold]);

  return { scrolled, direction };
};
