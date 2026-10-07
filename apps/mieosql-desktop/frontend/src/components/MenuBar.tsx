import { useEffect, useRef, useState } from "react";

import { useStore } from "../lib/store";

type MenuItem =
  | { kind: "separator" }
  | {
      kind: "item";
      label: string;
      shortcut?: string;
      disabled?: boolean;
      onClick: () => void;
    };

function Menu({ label, items }: { label: string; items: MenuItem[] }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <span
        className={`cursor-default rounded px-1.5 py-0.5 ${
          open ? "bg-blue-100 text-blue-800" : "hover:bg-blue-100 hover:text-blue-800"
        }`}
        onClick={() => setOpen((v) => !v)}
      >
        {label}
      </span>
      {open && (
        <div className="absolute left-0 z-50 mt-1 min-w-[220px] rounded border border-slate-300 bg-white py-1 shadow-lg">
          {items.map((item, i) =>
            item.kind === "separator" ? (
              <div key={i} className="my-1 h-px bg-slate-200" />
            ) : (
              <button
                key={i}
                disabled={item.disabled}
                className={`flex w-full items-center justify-between px-3 py-1 text-left text-[11px] ${
                  item.disabled
                    ? "cursor-not-allowed text-slate-300"
                    : "text-slate-700 hover:bg-sky-50 hover:text-sky-800"
                }`}
                onClick={() => {
                  setOpen(false);
                  item.onClick();
                }}
              >
                <span>{item.label}</span>
                {item.shortcut && (
                  <span className="pl-4 font-code text-[10px] text-slate-400">
                    {item.shortcut}
                  </span>
                )}
              </button>
            ),
          )}
        </div>
      )}
    </div>
  );
}

export function MenuBar() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const activeSessionId = activeTab?.sessionId ?? store.activeSessionId;
  const session = store.sessions.find((s) => s.id === activeSessionId);

  const dispatch = (name: string) => window.dispatchEvent(new CustomEvent(name));

  return (
    <nav className="flex h-[22px] shrink-0 items-center gap-3 border-b border-[#cbd5e1] bg-[#f8fafc] px-2 text-[11px] text-slate-700 select-none">
      <Menu
        label="File"
        items={[
          {
            kind: "item",
            label: "New Connection…",
            onClick: () => store.setConnectionModal(true),
          },
          {
            kind: "item",
            label: "New Query",
            shortcut: "Ctrl+N",
            onClick: () => store.newQueryTab(),
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Refresh Connections",
            onClick: () => {
              void store.refreshProfiles();
              dispatch("mieosql:refresh-tree");
            },
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Export Result (CSV)…",
            disabled: !activeTab || activeTab.results.length === 0,
            onClick: () => {
              if (activeTab) {
                void store.exportResult(activeTab.id, activeTab.activeResult, "csv");
              }
            },
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Exit",
            onClick: () =>
              window.dispatchEvent(new CustomEvent("mieosql:exit")),
          },
        ]}
      />
      <Menu
        label="Edit"
        items={[
          {
            kind: "item",
            label: "Clear Editor",
            disabled: activeTab?.kind !== "query",
            onClick: () => {
              if (activeTab) store.setSql(activeTab.id, "");
              dispatch("mieosql:clear-editor");
            },
          },
          {
            kind: "item",
            label: "Focus Editor",
            disabled: activeTab?.kind !== "query",
            onClick: () => dispatch("mieosql:focus-editor"),
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Format SQL",
            shortcut: "Shift+Alt+F",
            disabled: activeTab?.kind !== "query",
            onClick: () => dispatch("mieosql:format-sql"),
          },
          {
            kind: "item",
            label: "Minify SQL",
            disabled: activeTab?.kind !== "query",
            onClick: () => dispatch("mieosql:minify-sql"),
          },
          {
            kind: "item",
            label: "Rename Symbol…",
            shortcut: "F2",
            disabled: activeTab?.kind !== "query",
            onClick: () => dispatch("mieosql:rename-symbol"),
          },
        ]}
      />
      <Menu
        label="View"
        items={[
          {
            kind: "item",
            label: "Expand Editor",
            onClick: () => dispatch("mieosql:split-editor"),
          },
          {
            kind: "item",
            label: "Expand Results",
            onClick: () => dispatch("mieosql:split-results"),
          },
          {
            kind: "item",
            label: "Reset Layout",
            onClick: () => dispatch("mieosql:split-reset"),
          },
        ]}
      />
      <Menu
        label="Tools"
        items={[
          {
            kind: "item",
            label: "Begin Transaction",
            disabled: !session,
            onClick: () => {
              if (session) void store.beginTransaction(session.id);
            },
          },
          {
            kind: "item",
            label: "Commit",
            disabled: !session?.in_transaction,
            onClick: () => {
              if (session) void store.commitTransaction(session.id);
            },
          },
          {
            kind: "item",
            label: "Rollback",
            disabled: !session?.in_transaction,
            onClick: () => {
              if (session) void store.rollbackTransaction(session.id);
            },
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Find in Database…",
            shortcut: "Ctrl+Shift+F",
            onClick: () => store.openFeatureDialog("search"),
          },
          {
            kind: "item",
            label: "Generate Test Data…",
            disabled: !session,
            onClick: () => store.openFeatureDialog("datagen"),
          },
          {
            kind: "item",
            label: "Export with Data Masking…",
            disabled: !activeTab || activeTab.results.length === 0,
            onClick: () => store.openFeatureDialog("mask"),
          },
          {
            kind: "item",
            label: "Data Dictionary…",
            disabled: !session,
            onClick: () => store.openFeatureDialog("docs"),
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Structure Synchronization…",
            onClick: () => store.openFeatureDialog("structure-sync"),
          },
          {
            kind: "item",
            label: "Data Synchronization…",
            onClick: () => store.openFeatureDialog("data-sync"),
          },
          {
            kind: "item",
            label: "Data Transfer…",
            onClick: () => store.openFeatureDialog("transfer"),
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "ER Diagram",
            disabled: !session,
            onClick: () => {
              if (session)
                store.openFeatureTab("erd", "ER Diagram", {
                  sessionId: session.id,
                });
            },
          },
          {
            kind: "item",
            label: "BI Dashboard",
            onClick: () => store.openFeatureTab("bi", "Dashboard"),
          },
          { kind: "separator" },
          {
            kind: "item",
            label: "Team Workspace (share connections)…",
            onClick: () => store.openFeatureDialog("workspace"),
          },
        ]}
      />
      <Menu
        label="Help"
        items={[
          {
            kind: "item",
            label: `About MieoSQL (backend ${store.apiVersion || "…"})`,
            onClick: () =>
              store.toast(
                "info",
                `MieoSQL backend ${store.apiVersion} — Rust + Tauri 2`,
              ),
          },
          {
            kind: "item",
            label: "Keyboard Shortcuts",
            onClick: () =>
              store.toast(
                "info",
                "Ctrl+Enter: run · Ctrl+N: new query · Ctrl+C on a cell: copy",
              ),
          },
        ]}
      />
    </nav>
  );
}
