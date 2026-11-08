import { useEffect, useState } from "react";

/**
 * Whether the window is on screen. With `graceMs`, it counts as visible until it has been
 * hidden that long, so a quick switch to another app changes nothing.
 */
export function useWindowVisible(graceMs = 0): boolean {
  const [visible, setVisible] = useState(() => document.visibilityState !== "hidden");

  useEffect(() => {
    let timer: number | undefined;
    const update = () => {
      window.clearTimeout(timer);
      if (document.visibilityState !== "hidden") {
        setVisible(true);
      } else if (graceMs > 0) {
        timer = window.setTimeout(() => setVisible(false), graceMs);
      } else {
        setVisible(false);
      }
    };
    document.addEventListener("visibilitychange", update);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", update);
    };
  }, [graceMs]);

  return visible;
}
