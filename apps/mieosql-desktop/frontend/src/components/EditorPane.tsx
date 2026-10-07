import { MySQL, PostgreSQL, SQLite, sql } from "@codemirror/lang-sql";
import { Compartment, EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";

import type { Engine, Scope } from "../lib/bindings";

export type EditorHandle = {
  getText: () => string;
  setText: (text: string) => void;
  focus: () => void;
};

function dialectFor(engine: Engine | null | undefined) {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return MySQL;
    case "sqlite":
      return SQLite;
    default:
      return PostgreSQL;
  }
}

type Props = {
  initialDoc: string;
  engine: Engine | null;
  /** Session the tab is bound to (schema-aware insight uses this). */
  sessionId?: string | null;
  /** Scope of the tab, when known (schema-aware insight uses this). */
  scope?: Scope | null;
  onRun: () => void;
  onChange: (text: string) => void;
  onCursor: (line: number, col: number) => void;
};

export const EditorPane = forwardRef<EditorHandle, Props>(function EditorPane(
  { initialDoc, engine, sessionId, scope, onRun, onChange, onCursor },
  ref,
) {
  // Reserved for the Code Insight workstream (completion/hover/rename).
  void sessionId;
  void scope;
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const dialectCompartment = useRef(new Compartment());
  const callbacks = useRef({ onRun, onChange, onCursor });
  callbacks.current = { onRun, onChange, onCursor };
  const engineRef = useRef(engine);
  engineRef.current = engine;
  const initialDocRef = useRef(initialDoc);

  useEffect(() => {
    if (!hostRef.current) return;
    const view = new EditorView({
      parent: hostRef.current,
      state: EditorState.create({
        doc: initialDocRef.current,
        extensions: [
          basicSetup,
          dialectCompartment.current.of(
            sql({ dialect: dialectFor(engineRef.current) }),
          ),
          Prec.highest(
            keymap.of([
              {
                key: "Mod-Enter",
                run: () => {
                  callbacks.current.onRun();
                  return true;
                },
              },
            ]),
          ),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) callbacks.current.onChange(u.state.doc.toString());
            if (u.selectionSet || u.docChanged) {
              const pos = u.state.selection.main.head;
              const line = u.state.doc.lineAt(pos);
              callbacks.current.onCursor(line.number, pos - line.from + 1);
            }
          }),
        ],
      }),
    });
    viewRef.current = view;
    callbacks.current.onCursor(1, 1);
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: dialectCompartment.current.reconfigure(
        sql({ dialect: dialectFor(engine) }),
      ),
    });
  }, [engine]);

  useImperativeHandle(
    ref,
    () => ({
      getText: () => viewRef.current?.state.doc.toString() ?? "",
      setText: (text: string) => {
        const view = viewRef.current;
        if (!view) return;
        view.dispatch({
          changes: { from: 0, to: view.state.doc.length, insert: text },
        });
      },
      focus: () => viewRef.current?.focus(),
    }),
    [],
  );

  return <div ref={hostRef} className="h-full min-h-0 overflow-hidden" />;
});
