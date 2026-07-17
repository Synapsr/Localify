"use client";

import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import { useEffect, useRef } from "react";

import type { TerminalSessionDto } from "@/types/localify";

import { cn } from "@/lib/utils";
import { useTerminal } from "@/providers/terminal-provider";

interface TerminalInstanceProps {
  active: boolean;
  chunks: Uint8Array[];
  session: TerminalSessionDto;
}

export const TerminalInstance = ({ active, chunks, session }: TerminalInstanceProps) => {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const consumedChunkCountRef = useRef(0);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const terminalRef = useRef<XTerm | null>(null);
  const { resizeSession, writeToSession } = useTerminal();

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const terminal = new XTerm({
      allowTransparency: true,
      convertEol: true,
      cursorBlink: true,
      fontFamily: "'Geist Mono Variable', 'Geist Mono', monospace",
      fontSize: 12,
      lineHeight: 1.34,
      macOptionIsMeta: true,
      scrollback: 5000,
      theme: {
        background: "#101010",
        black: "#101010",
        blue: "#7aa2f7",
        brightBlack: "#6b7280",
        brightBlue: "#a5b4fc",
        brightCyan: "#67e8f9",
        brightGreen: "#86efac",
        brightMagenta: "#f0abfc",
        brightRed: "#fca5a5",
        brightWhite: "#ffffff",
        brightYellow: "#fde68a",
        cursor: "#f5f5f5",
        cyan: "#22d3ee",
        foreground: "#dddddd",
        green: "#4ade80",
        magenta: "#e879f9",
        red: "#f87171",
        selectionBackground: "#3a3a3a",
        white: "#e5e5e5",
        yellow: "#facc15",
      },
    });
    const fitAddon = new FitAddon();

    terminal.loadAddon(fitAddon);
    terminal.open(container);
    terminal.focus();

    const dataDisposable = terminal.onData((data) => {
      void writeToSession(session.id, data);
    });

    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;

    const resizeObserver = new ResizeObserver(() => {
      fitAndSync(session.id, fitAddon, resizeSession);
    });
    resizeObserver.observe(container);
    fitAndSync(session.id, fitAddon, resizeSession);

    return () => {
      resizeObserver.disconnect();
      dataDisposable.dispose();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
    };
  }, [resizeSession, session.id, writeToSession]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) return;

    if (consumedChunkCountRef.current > chunks.length) {
      consumedChunkCountRef.current = 0;
    }

    for (let index = consumedChunkCountRef.current; index < chunks.length; index += 1) {
      terminal.write(chunks[index]);
    }

    consumedChunkCountRef.current = chunks.length;
  }, [chunks]);

  useEffect(() => {
    if (!active) return;
    const terminal = terminalRef.current;
    const fitAddon = fitAddonRef.current;
    if (!terminal || !fitAddon) return;

    requestAnimationFrame(() => {
      fitAndSync(session.id, fitAddon, resizeSession);
      terminal.focus();
    });
  }, [active, resizeSession, session.id]);

  return (
    <div
      ref={containerRef}
      className={cn(
        "localify-terminal h-full min-h-0 w-full overflow-hidden bg-[#101010]",
        active ? "block" : "hidden",
      )}
    />
  );
};

function fitAndSync(
  sessionId: string,
  fitAddon: FitAddon,
  resizeSession: (sessionId: string, cols: number, rows: number) => Promise<void>,
) {
  try {
    fitAddon.fit();
    const dimensions = fitAddon.proposeDimensions();
    if (!dimensions) return;
    void resizeSession(sessionId, dimensions.cols, dimensions.rows);
  } catch {
    // xterm can throw while the element is hidden during tab switches.
  }
}
