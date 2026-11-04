"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

const TARGET_WIDTH = 960;

export const AppWindow = ({ children }: { children: ReactNode }) => {
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(1);
  const [contentHeight, setContentHeight] = useState<number | null>(null);

  useEffect(() => {
    const viewport = viewportRef.current;
    const content = contentRef.current;
    if (!viewport || !content) return;

    const updateScale = () => {
      const width = viewport.clientWidth;
      if (width > 0) setScale(Math.min(1, width / TARGET_WIDTH));
    };
    const updateHeight = () => {
      setContentHeight(content.offsetHeight);
    };

    updateScale();
    updateHeight();

    const roViewport = new ResizeObserver(updateScale);
    roViewport.observe(viewport);
    const roContent = new ResizeObserver(updateHeight);
    roContent.observe(content);

    return () => {
      roViewport.disconnect();
      roContent.disconnect();
    };
  }, []);

  return (
    <div className="w-full max-w-[960px] overflow-hidden rounded-md border bg-neutral-50 p-0.5 pt-0 shadow-[0_1px_0_rgba(13,13,13,0.03),0_40px_80px_-30px_rgba(13,13,13,0.35)] md:rounded-[14px] md:p-1.5 dark:bg-neutral-950/40">
      <div className="flex items-center gap-1.5 px-0.5 py-1 md:px-1.5 md:py-2.5">
        <span className="size-2 rounded-full bg-[#FF5C5F] md:size-3" />
        <span className="size-2 rounded-full bg-[#FAC800] md:size-3" />
        <span className="size-2 rounded-full bg-[#34C759] md:size-3" />
      </div>
      <div
        ref={viewportRef}
        className="overflow-hidden rounded-md bg-background md:rounded-lg"
        style={{ height: contentHeight != null ? contentHeight * scale : undefined }}
      >
        <div
          ref={contentRef}
          style={{
            width: TARGET_WIDTH,
            transform: `scale(${scale})`,
            transformOrigin: "top left",
            willChange: "transform",
          }}
        >
          {children}
        </div>
      </div>
    </div>
  );
};
