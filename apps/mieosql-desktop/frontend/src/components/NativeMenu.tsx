// macOS application menu (native NSMenu) — mirrors MenuBar.tsx so macOS gets
// the real system menu bar instead of an in-window row. No DOM rendered.

import { Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { useEffect } from "react";

import { isSqlEngine } from "../lib/quoting";
import { runScriptFile } from "../lib/run-script";
import { useStore } from "../lib/store";
import { openSqlFile, saveActiveQuery } from "./MenuBar";

export function NativeMenu() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const session = store.sessions.find(
    (s) => s.id === (activeTab?.sessionId ?? store.activeSessionId),
  );
  const sessionId = session?.id ?? null;
  const engine = session?.engine ?? null;
  const canCreateDatabase =
    !!sessionId && engine !== null && isSqlEngine(engine) && engine !== "sqlite";
  const inTx = session?.in_transaction ?? false;
  const kind = activeTab?.kind ?? null;
  const resultsCount = activeTab?.results.length ?? 0;
  const apiVersion = store.apiVersion;

  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const dispatch = (name: string) => window.dispatchEvent(new CustomEvent(name));
        const item = (
          text: string,
          action: () => void,
          options?: { accelerator?: string; enabled?: boolean },
        ) =>
          MenuItem.new({
            text,
            enabled: options?.enabled ?? true,
            accelerator: options?.accelerator,
            action,
          });

        const sep = () => PredefinedMenuItem.new({ item: "Separator" });

        const sessionNow = () =>
          useStore
            .getState()
            .sessions.find((s) => s.id === sessionId) ?? null;

        const appMenu = await Submenu.new({
          text: "MieoSQL",
          items: [
            await item("About MieoSQL", () =>
              useStore
                .getState()
                .toast("info", `MieoSQL backend ${apiVersion || "…"} — Rust + Tauri 2`),
            ),
            await sep(),
            await PredefinedMenuItem.new({ item: "Services" }),
            await sep(),
            await PredefinedMenuItem.new({ item: "Hide", text: "Hide MieoSQL" }),
            await PredefinedMenuItem.new({ item: "HideOthers" }),
            await PredefinedMenuItem.new({ item: "ShowAll" }),
            await sep(),
            await PredefinedMenuItem.new({ item: "Quit", text: "Quit MieoSQL" }),
          ],
        });

        const fileMenu = await Submenu.new({
          text: "File",
          items: [
            await item("New Connection…", () =>
              useStore.getState().setConnectionModal(true),
            ),
            await item(
              "New Query",
              () => useStore.getState().newQueryTab(),
              { accelerator: "CmdOrCtrl+N" },
            ),
            await sep(),
            await item("Save", () => void saveActiveQuery(false), {
              accelerator: "CmdOrCtrl+S",
              enabled: kind === "query",
            }),
            await item("Save As…", () => void saveActiveQuery(true), {
              enabled: kind === "query",
            }),
            await item("Open .sql in Editor…", () => openSqlFile()),
            await sep(),
            await item("Refresh Connections", () => {
              void useStore.getState().refreshProfiles();
              dispatch("mieosql:refresh-tree");
            }),
            await sep(),
            await item(
              "New Database…",
              () => useStore.getState().openObjectDialog("new-database", { sessionId }),
              { enabled: canCreateDatabase },
            ),
            await item(
              "Backup Database…",
              () => useStore.getState().openObjectDialog("backup", { sessionId, scope: {} }),
              { enabled: !!sessionId },
            ),
            await item(
              "Restore from SQL File…",
              () => useStore.getState().openObjectDialog("restore", { sessionId }),
              { enabled: !!sessionId },
            ),
            await sep(),
            await item("Open SQL File…", () => void runScriptFile(), {
              enabled: !!sessionId,
            }),
            await sep(),
            await item(
              "Export Result (CSV)…",
              () => {
                const st = useStore.getState();
                const tab = st.tabs.find((t) => t.id === st.activeTabId);
                if (tab) void st.exportResult(tab.id, tab.activeResult, "csv");
              },
              { enabled: resultsCount > 0 },
            ),
            await sep(),
            await PredefinedMenuItem.new({ item: "CloseWindow" }),
          ],
        });

        const editMenu = await Submenu.new({
          text: "Edit",
          items: [
            await PredefinedMenuItem.new({ item: "Undo" }),
            await PredefinedMenuItem.new({ item: "Redo" }),
            await sep(),
            await PredefinedMenuItem.new({ item: "Cut" }),
            await PredefinedMenuItem.new({ item: "Copy" }),
            await PredefinedMenuItem.new({ item: "Paste" }),
            await PredefinedMenuItem.new({ item: "SelectAll" }),
            await sep(),
            await item(
              "Clear Editor",
              () => {
                const st = useStore.getState();
                if (st.activeTabId) st.setSql(st.activeTabId, "");
                dispatch("mieosql:clear-editor");
              },
              { enabled: kind === "query" },
            ),
            await item(
              "Focus Editor",
              () => dispatch("mieosql:focus-editor"),
              { enabled: kind === "query" },
            ),
            await sep(),
            await item("Format SQL", () => dispatch("mieosql:format-sql"), {
              accelerator: "Shift+Alt+F",
              enabled: kind === "query",
            }),
            await item("Minify SQL", () => dispatch("mieosql:minify-sql"), {
              enabled: kind === "query",
            }),
            await item("Fix SQL Syntax", () => dispatch("mieosql:fix-sql"), {
              enabled: kind === "query",
            }),
            await item(
              "Rename Symbol…",
              () => dispatch("mieosql:rename-symbol"),
              { accelerator: "F2", enabled: kind === "query" },
            ),
          ],
        });

        const viewMenu = await Submenu.new({
          text: "View",
          items: [
            await item("Expand Editor", () => dispatch("mieosql:split-editor")),
            await item("Expand Results", () => dispatch("mieosql:split-results")),
            await item("Reset Layout", () => dispatch("mieosql:split-reset")),
          ],
        });

        const toolsMenu = await Submenu.new({
          text: "Tools",
          items: [
            await item(
              "Begin Transaction",
              () => {
                const s = sessionNow();
                if (s) void useStore.getState().beginTransaction(s.id);
              },
              { enabled: !!sessionId && !inTx },
            ),
            await item(
              "Commit Transaction",
              () => {
                const s = sessionNow();
                if (s) void useStore.getState().commitTransaction(s.id);
              },
              { enabled: !!sessionId && inTx },
            ),
            await item(
              "Rollback Transaction",
              () => {
                const s = sessionNow();
                if (s) void useStore.getState().rollbackTransaction(s.id);
              },
              { enabled: !!sessionId && inTx },
            ),
            await sep(),
            await item(
              "Find in Database…",
              () => useStore.getState().openFeatureDialog("search"),
              { accelerator: "Shift+CmdOrCtrl+F" },
            ),
            await item(
              "Generate Test Data…",
              () => useStore.getState().openFeatureDialog("datagen"),
              { enabled: !!sessionId },
            ),
            await item(
              "Export with Data Masking…",
              () => useStore.getState().openFeatureDialog("mask"),
              { enabled: resultsCount > 0 },
            ),
            await item(
              "Data Dictionary…",
              () => useStore.getState().openFeatureDialog("docs"),
              { enabled: !!sessionId },
            ),
            await sep(),
            await item("Structure Synchronization…", () =>
              useStore.getState().openFeatureDialog("structure-sync"),
            ),
            await item("Data Synchronization…", () =>
              useStore.getState().openFeatureDialog("data-sync"),
            ),
            await item("Data Transfer…", () =>
              useStore.getState().openFeatureDialog("transfer"),
            ),
            await sep(),
            await item(
              "ER Diagram",
              () => {
                if (sessionId)
                  useStore
                    .getState()
                    .openFeatureTab("erd", "ER Diagram", { sessionId });
              },
              { enabled: !!sessionId },
            ),
            await item("BI Dashboard", () =>
              useStore.getState().openFeatureTab("bi", "Dashboard"),
            ),
            await sep(),
            await item("Team Workspace (share connections)…", () =>
              useStore.getState().openFeatureDialog("workspace"),
            ),
          ],
        });

        const helpMenu = await Submenu.new({
          text: "Help",
          items: [
            await item("Keyboard Shortcuts", () =>
              useStore
                .getState()
                .toast(
                  "info",
                  "⌘Enter: run · ⌘N: new query · ⇧⌘F: find in database · ⌘C on a cell: copy",
                ),
            ),
          ],
        });

        const menu = await Menu.new({
          items: [appMenu, fileMenu, editMenu, viewMenu, toolsMenu, helpMenu],
        });
        if (!disposed) await menu.setAsAppMenu();
      } catch (err) {
        // Native menu is an enhancement: the ribbon and shortcuts still work.
        console.warn("Native menu unavailable:", err);
      }
    })();
    return () => {
      disposed = true;
    };
  }, [sessionId, engine, inTx, kind, resultsCount, apiVersion]);

  return null;
}
