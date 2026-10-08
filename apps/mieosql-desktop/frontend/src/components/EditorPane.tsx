import { MSSQL, MySQL, PostgreSQL, SQLite, sql } from "@codemirror/lang-sql";
import {
  Compartment,
  EditorState,
  Prec,
  type Extension,
} from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { syntaxHighlighting } from "@codemirror/language";

import type { Engine, Scope } from "../lib/bindings";
import { mieosqlDark, mieosqlLight } from "../lib/editor-theme";
import { resolveTheme } from "../lib/theme";
import { useStore } from "../lib/store";
import {
  formatSql,
  identifierAt,
  identifierOccurrences,
  minifySql,
} from "../lib/sql-format";
import { insightExtensions } from "../lib/sql-insight";

export type EditorHandle = {
  getText: () => string;
  setText: (text: string) => void;
  focus: () => void;
};

/**
 * Language support for the editor. SQL engines get a matching dialect;
 * document / key-value / broker engines (MongoDB, Redis, Kafka) use native
 * query languages, so they stay plain text (no `sql()` extension).
 */
function dialectExtension(engine: Engine | null | undefined): Extension {
  switch (engine) {
    case "mysql":
    case "mariadb":
      return sql({ dialect: MySQL });
    case "sqlite":
      return sql({ dialect: SQLite });
    case "sql_server":
      return sql({ dialect: MSSQL });
    case "mongodb":
    case "redis":
    case "kafka":
      return [];
    default:
      return sql({ dialect: PostgreSQL });
  }
}

/**
 * Replace the whole document while keeping the cursor at (roughly) the same
 * line/column — used by beautify / minify.
 */
function applyDoc(view: EditorView, next: string): void {
  const { state } = view;
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  const lineNumber = line.number;
  const col = head - line.from;
  const lines = next.split("\n");
  const targetLine = Math.min(lineNumber, lines.length);
  let pos = 0;
  for (let i = 0; i < targetLine - 1; i++) pos += (lines[i]?.length ?? 0) + 1;
  pos += Math.min(col, lines[targetLine - 1]?.length ?? 0);
  view.dispatch({
    changes: { from: 0, to: state.doc.length, insert: next },
    selection: { anchor: Math.min(pos, next.length) },
    scrollIntoView: true,
  });
}

/**
 * Rename the identifier at the selection/cursor: prompts for a new name and
 * replaces every word-boundary occurrence (case-sensitive) in one transaction.
 * Strings, quoted identifiers and comments are skipped.
 */
function renameSymbol(view: EditorView, engine: Engine | null): void {
  const { state } = view;
  const selection = state.selection.main;
  const text = state.doc.toString();
  let word: string | null = null;
  if (!selection.empty) {
    const selected = state.doc.sliceString(selection.from, selection.to).trim();
    if (/^[A-Za-z_][A-Za-z0-9_$]*$/.test(selected)) word = selected;
  } else {
    word = identifierAt(engine, text, selection.head)?.text ?? null;
  }
  if (!word) return;

  const next = window.prompt(`Rename "${word}" to:`, word);
  if (!next || next === word) return;
  if (!/^[A-Za-z_][A-Za-z0-9_$]*$/.test(next)) {
    window.alert(`"${next}" is not a valid SQL identifier.`);
    return;
  }
  const occurrences = identifierOccurrences(engine, text, word);
  if (occurrences.length === 0) return;
  view.dispatch({
    changes: occurrences.map((o) => ({ from: o.from, to: o.to, insert: next })),
    userEvent: "rename",
  });
  view.focus();
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
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const dialectCompartment = useRef(new Compartment());
  const insightCompartment = useRef(new Compartment());
  const themeCompartment = useRef(new Compartment());
  const themeSetting = useStore((s) => s.settings.theme);
  const [isDark, setIsDark] = useState(
    () =>
      typeof window !== "undefined" &&
      resolveTheme(themeSetting) === "dark",
  );
  const callbacks = useRef({ onRun, onChange, onCursor });
  callbacks.current = { onRun, onChange, onCursor };
  const engineRef = useRef(engine);
  engineRef.current = engine;
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;
  const scopeRef = useRef(scope);
  scopeRef.current = scope;
  const initialDocRef = useRef(initialDoc);

  useEffect(() => {
    if (!hostRef.current) return;
    const formatNow = (view: EditorView) =>
      applyDoc(view, formatSql(engineRef.current, view.state.doc.toString()));
    const minifyNow = (view: EditorView) =>
      applyDoc(view, minifySql(engineRef.current, view.state.doc.toString()));
    const renameNow = (view: EditorView) => renameSymbol(view, engineRef.current);

    const view = new EditorView({
      parent: hostRef.current,
      state: EditorState.create({
        doc: initialDocRef.current,
        extensions: [
          basicSetup,
          themeCompartment.current.of(
            syntaxHighlighting(isDark ? mieosqlDark : mieosqlLight),
          ),
          dialectCompartment.current.of(dialectExtension(engineRef.current)),
          insightCompartment.current.of(
            insightExtensions(engineRef.current, sessionRef.current, () => scopeRef.current),
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
              {
                key: "Shift-Alt-f",
                run: (v) => {
                  formatNow(v);
                  return true;
                },
              },
              {
                key: "Shift-Alt-m",
                run: (v) => {
                  minifyNow(v);
                  return true;
                },
              },
              {
                key: "F2",
                run: (v) => {
                  renameNow(v);
                  return true;
                },
              },
            ]),
          ),
          // macOS turns Option+Shift+F/M into special characters, so the keymap
          // above never sees them — catch the physical keys here instead.
          EditorView.domEventHandlers({
            keydown: (event, v) => {
              if (event.defaultPrevented) return false;
              if (!event.altKey || !event.shiftKey || event.ctrlKey || event.metaKey) {
                return false;
              }
              if (event.code === "KeyF") {
                formatNow(v);
                return true;
              }
              if (event.code === "KeyM") {
                minifyNow(v);
                return true;
              }
              return false;
            },
          }),
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
        dialectExtension(engine),
      ),
    });
  }, [engine]);

  // Rebuild schema-aware completion/hover when the connection or scope changes.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: insightCompartment.current.reconfigure(
        insightExtensions(engine, sessionId, () => scopeRef.current),
      ),
    });
  }, [engine, sessionId, scope?.database, scope?.schema]);

  // Follow the app theme (Settings → system/light/dark) for SQL colors.
  useEffect(() => {
    setIsDark(resolveTheme(themeSetting) === "dark");
    if (themeSetting !== "system") return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setIsDark(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [themeSetting]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: themeCompartment.current.reconfigure(
        syntaxHighlighting(isDark ? mieosqlDark : mieosqlLight),
      ),
    });
  }, [isDark]);

  // Menu/toolbar events (Edit → Format/Minify/Rename).
  useEffect(() => {
    const onFormat = () => {
      const view = viewRef.current;
      if (!view) return;
      applyDoc(view, formatSql(engineRef.current, view.state.doc.toString()));
    };
    const onMinify = () => {
      const view = viewRef.current;
      if (!view) return;
      applyDoc(view, minifySql(engineRef.current, view.state.doc.toString()));
    };
    const onRename = () => {
      const view = viewRef.current;
      if (view) renameSymbol(view, engineRef.current);
    };
    window.addEventListener("mieosql:format-sql", onFormat);
    window.addEventListener("mieosql:minify-sql", onMinify);
    window.addEventListener("mieosql:rename-symbol", onRename);
    return () => {
      window.removeEventListener("mieosql:format-sql", onFormat);
      window.removeEventListener("mieosql:minify-sql", onMinify);
      window.removeEventListener("mieosql:rename-symbol", onRename);
    };
  }, []);

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
