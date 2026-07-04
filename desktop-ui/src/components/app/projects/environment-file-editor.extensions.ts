import {
  HighlightStyle,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

export const environmentFileLanguage = StreamLanguage.define({
  ...properties,
  tokenTable: {
    quote: tags.string,
  },
});

const environmentFileHighlightStyle = HighlightStyle.define([
  {
    tag: [tags.definition(tags.variableName), tags.propertyName],
    color: "var(--env-editor-key-color)",
    fontWeight: "500",
  },
  {
    tag: tags.string,
    color: "var(--env-editor-value-color)",
  },
  {
    tag: tags.comment,
    color: "var(--env-editor-comment-color)",
  },
  {
    tag: tags.heading,
    color: "var(--env-editor-section-color)",
    fontWeight: "500",
  },
]);

export const environmentFileHighlighting = syntaxHighlighting(
  environmentFileHighlightStyle,
);

export const environmentFileTheme = EditorView.theme({
  "&": {
    "--env-editor-comment-color":
      "color-mix(in oklab, var(--chart-5) 62%, var(--muted-foreground))",
    "--env-editor-key-color":
      "color-mix(in oklab, var(--ring) 58%, var(--foreground))",
    "--env-editor-section-color":
      "color-mix(in oklab, var(--chart-5) 70%, var(--foreground))",
    "--env-editor-value-color": "var(--foreground)",
    height: "100%",
    backgroundColor: "transparent",
    color: "var(--foreground)",
    fontSize: "12.5px",
  },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.62",
  },
  ".cm-content": {
    minHeight: "100%",
    padding: "16px 0 20px",
  },
  ".cm-line": {
    padding: "0 18px",
  },
  ".cm-gutters": {
    backgroundColor: "color-mix(in oklab, var(--muted) 42%, transparent)",
    borderRight: "0",
    color: "var(--muted-foreground)",
    boxShadow: "inset -1px 0 color-mix(in oklab, var(--foreground) 8%, transparent)",
    fontVariantNumeric: "tabular-nums",
  },
  ".cm-lineNumbers .cm-gutterElement": {
    minWidth: "42px",
    padding: "0 12px 0 8px",
    textAlign: "right",
  },
  ".cm-gutterElement": {
    opacity: "0.72",
  },
  ".cm-gutterElement.cm-activeLineGutter": {
    opacity: "1",
  },
  ".cm-activeLine": {
    backgroundColor: "color-mix(in oklab, var(--muted) 48%, transparent)",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "color-mix(in oklab, var(--muted) 58%, transparent)",
    color: "var(--foreground)",
  },
  ".cm-selectionBackground": {
    backgroundColor: "color-mix(in oklab, var(--primary) 20%, transparent) !important",
  },
  ".cm-matchingBracket": {
    backgroundColor: "color-mix(in oklab, var(--primary) 12%, transparent)",
    outline: "1px solid color-mix(in oklab, var(--primary) 18%, transparent)",
  },
  "&.cm-focused": {
    outline: "none",
  },
  ".cm-cursor": {
    borderLeftColor: "var(--foreground)",
  },
  ".cm-placeholder": {
    color: "var(--muted-foreground)",
  },
});
