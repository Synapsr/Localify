import { RiClaudeFill, RiCursorAiFill, RiOpenaiFill } from "@remixicon/react";

/* The coding agents Localify connects to, with their marks. Names are brands: never translated. */
export const AGENTS = [
  { name: "Claude Code", Logo: RiClaudeFill, color: "#D97757" },
  { name: "Codex", Logo: RiOpenaiFill, color: "currentColor" },
  { name: "Cursor", Logo: RiCursorAiFill, color: "currentColor" },
] as const;
