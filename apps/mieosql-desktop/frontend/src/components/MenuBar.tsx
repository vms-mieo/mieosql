import { save } from "@tauri-apps/plugin-dialog";

import { api, errorText } from "../lib/api";
import { isSqlEngine } from "../lib/quoting";
import { runScriptFile } from "../lib/run-script";
import { useStore } from "../lib/store";
import { PopupMenu, type MenuItem } from "./PopupMenu";

/** Last path each query tab was saved to (module-level: survives rerenders). */
const savedPaths = new Map<string, string>();

function activeQuerySql(): { tabId: string; title: string; sql: string } | null {
  const st = useStore.getState();
  const tab = st.tabs.find((t) => t.id === st.activeTabId);
  if (!tab || tab.kind !== "query") return null;
  return { tabId: tab.id, title: tab.title || "query", sql: tab.sql };
}

/** Save the active query tab to a .sql file (prompt when needed). */
export async function saveActiveQuery(saveAs: boolean): Promise<void> {
  const st = useStore.getState();
  const active = activeQuerySql();
  if (!active) {
    st.toast("info", "No query tab is active");
    return;
  }
  let path = !saveAs ? (savedPaths.get(active.tabId) ?? null) : null;
  if (!path) {
    const safe = active.title.replace(/[^\w.-]+/g, "_").slice(0, 60) || "query";
    try {
      path = await save({
        title: "Save SQL",
        defaultPath: `${safe}.sql`,
        filters: [{ name: "SQL", extensions: ["sql"] }],
      });
    } catch (err) {
      st.toast("error", errorText(err));
      return;
    }
    if (!path) return; // cancelled
  }
  try {
    await api.writeTextFile(path, active.sql);
    savedPaths.set(active.tabId, path);
    st.toast("success", `Saved ${path}`);
  } catch (err) {
    st.toast("error", errorText(err));
  }
}

/** Open a user-picked .sql/.txt file in a new query tab (FileReader). */
export function openSqlFile(): void {
  const st = useStore.getState();
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".sql,.txt,text/plain";
  input.onchange = () => {
    const file = input.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const text = typeof reader.result === "string" ? reader.result : "";
      const sid =
        st.activeSessionId ?? useStore.getState().sessions[0]?.id ?? null;
      useStore.getState().newQueryTab(sid, text);
      useStore.getState().toast("success", `Opened ${file.name}`);
    };
    reader.onerror = () => {
      useStore.getState().toast("error", `Could not read ${file.name}`);
    };
    reader.readAsText(file);
  };
  input.click();
}

/**
 * Classic menu bar (24px) — Apple-style dropdowns with accent highlight.
 */
export function MenuBar() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const activeSessionId = activeTab?.sessionId ?? store.activeSessionId;
  const session = store.sessions.find((s) => s.id === activeSessionId);

  const dispatch = (name: string) => window.dispatchEvent(new CustomEvent(name));

  const fileItems: MenuItem[] = [
    {
      kind: "item",
      label: "New Connection…",
      onClick: () => store.setConnectionModal(true),
    },
    {
      kind: "item",
      label: "New Query",
      shortcut: "⌘N",
      onClick: () => store.newQueryTab(),
    },
    { kind: "separator" },
    {
      kind: "item",
      label: "Save",
      shortcut: "⌘S",
      disabled: activeTab?.kind !== "query",
      onClick: () => void saveActiveQuery(false),
    },
    {
      kind: "item",
      label: "Save As…",
      disabled: activeTab?.kind !== "query",
      onClick: () => void saveActiveQuery(true),
    },
    {
      kind: "item",
      label: "Open .sql in Editor…",
      onClick: () => openSqlFile(),
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
      label: "New Database…",
      disabled: !session || !isSqlEngine(session.engine) || session.engine === "sqlite",
      onClick: () => {
        if (session) store.openObjectDialog("new-database", { sessionId: session.id });
      },
    },
    {
      kind: "item",
      label: "Backup Database…",
      disabled: !session,
      onClick: () => {
        if (session) store.openObjectDialog("backup", { sessionId: session.id, scope: {} });
      },
    },
    {
      kind: "item",
      label: "Restore from SQL File…",
      disabled: !session,
      onClick: () => {
        if (session) store.openObjectDialog("restore", { sessionId: session.id });
      },
    },
    { kind: "separator" },
    {
      kind: "item",
      label: "Open SQL File…",
      disabled: !session,
      onClick: () => void runScriptFile(),
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
      label: "Quit MieoSQL",
      shortcut: "⌘Q",
      onClick: () => window.dispatchEvent(new CustomEvent("mieosql:exit")),
    },
  ];

  const editItems: MenuItem[] = [
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
      shortcut: "⇧⌥F",
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
      label: "Fix SQL Syntax",
      disabled: activeTab?.kind !== "query",
      onClick: () => dispatch("mieosql:fix-sql"),
    },
    {
      kind: "item",
      label: "Rename Symbol…",
      shortcut: "F2",
      disabled: activeTab?.kind !== "query",
      onClick: () => dispatch("mieosql:rename-symbol"),
    },
  ];

  const viewItems: MenuItem[] = [
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
  ];

  const toolsItems: MenuItem[] = [
    { kind: "label", label: "Transaction" },
    {
      kind: "item",
      label: "Begin",
      disabled: !session || session.in_transaction,
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
    { kind: "label", label: "Tools" },
    {
      kind: "item",
      label: "Find in Database…",
      shortcut: "⇧⌘F",
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
          store.openFeatureTab("erd", "ER Diagram", { sessionId: session.id });
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
  ];

  const helpItems: MenuItem[] = [
    {
      kind: "item",
      label: "Keyboard Shortcuts",
      onClick: () =>
        store.toast(
          "info",
          "⌘Enter: run · ⌘N: new query · ⇧⌘F: find in database · ⌘C on a cell: copy",
        ),
    },
    {
      kind: "item",
      label: `About MieoSQL (backend ${store.apiVersion || "…"})`,
      onClick: () =>
        store.toast(
          "info",
          `MieoSQL backend ${store.apiVersion} — Rust + Tauri 2`,
        ),
    },
  ];

  const menuLabel = (label: string) =>
    ({ open, toggle }: { open: boolean; toggle: () => void }) => (
      <button
        className={`rounded-md px-2 py-[3px] text-[13px] transition-colors ${
          open
            ? "bg-slate-900/[0.07] text-slate-900"
            : "text-slate-700 hover:bg-slate-900/[0.05]"
        }`}
        onClick={toggle}
      >
        {label}
      </button>
    );

  return (
    <nav className="flex h-6 shrink-0 items-center gap-0.5 bg-window px-2 select-none">
      <PopupMenu items={fileItems} trigger={menuLabel("File")} />
      <PopupMenu items={editItems} trigger={menuLabel("Edit")} />
      <PopupMenu items={viewItems} trigger={menuLabel("View")} />
      <PopupMenu items={toolsItems} trigger={menuLabel("Tools")} />
      <PopupMenu items={helpItems} trigger={menuLabel("Help")} align="left" />
    </nav>
  );
}
