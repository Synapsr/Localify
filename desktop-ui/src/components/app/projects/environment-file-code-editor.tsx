"use client";

import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { useEffect, useMemo, useRef } from "react";

import {
  environmentFileHighlighting,
  environmentFileLanguage,
  environmentFileTheme,
} from "./environment-file-editor.extensions";

interface EnvironmentFileCodeEditorProps {
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
}

export const EnvironmentFileCodeEditor = ({
  value,
  disabled,
  onChange,
}: EnvironmentFileCodeEditorProps) => {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialValueRef = useRef(value);
  const disabledRef = useRef(disabled);
  const onChangeRef = useRef(onChange);
  const editableCompartment = useMemo(() => new Compartment(), []);
  const readOnlyCompartment = useMemo(() => new Compartment(), []);

  useEffect(() => {
    disabledRef.current = disabled;
    onChangeRef.current = onChange;
  }, [disabled, onChange]);

  useEffect(() => {
    const parent = containerRef.current;
    if (!parent) return;

    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: initialValueRef.current,
        extensions: [
          basicSetup,
          environmentFileLanguage,
          environmentFileHighlighting,
          environmentFileTheme,
          editableCompartment.of(EditorView.editable.of(!disabledRef.current)),
          readOnlyCompartment.of(EditorState.readOnly.of(disabledRef.current)),
          EditorView.lineWrapping,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              onChangeRef.current(update.state.doc.toString());
            }
          }),
        ],
      }),
    });

    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [editableCompartment, readOnlyCompartment]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;

    const currentValue = view.state.doc.toString();
    if (currentValue === value) return;

    view.dispatch({
      changes: {
        from: 0,
        to: view.state.doc.length,
        insert: value,
      },
    });
  }, [value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;

    view.dispatch({
      effects: [
        editableCompartment.reconfigure(EditorView.editable.of(!disabled)),
        readOnlyCompartment.reconfigure(EditorState.readOnly.of(disabled)),
      ],
    });
  }, [disabled, editableCompartment, readOnlyCompartment]);

  return <div ref={containerRef} className="h-full min-h-0" />;
};
