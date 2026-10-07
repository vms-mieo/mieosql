import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect } from "react";

import { api } from "./lib/api";
import { useStore } from "./lib/store";
import { ConnectionModal } from "./components/ConnectionModal";
import { MenuBar } from "./components/MenuBar";
import { Ribbon } from "./components/Ribbon";
import { RightPanel } from "./components/RightPanel";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Toasts } from "./components/Toasts";
import { Workspace } from "./components/Workspace";
import { FeatureDialogs } from "./features/dialogs";

export default function App() {
  const ready = useStore((s) => s.ready);

  useEffect(() => {
    void useStore.getState().init();
  }, []);

  // Stream query events into the store.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void api
      .onQueryEvent((queryId, event) =>
        useStore.getState().handleQueryEvent(queryId, event),
      )
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* running outside Tauri (browser preview) */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Global shortcuts: Ctrl+N new query.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && (e.key === "n" || e.key === "N")) {
        e.preventDefault();
        useStore.getState().newQueryTab();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Exit event (File → Exit).
  useEffect(() => {
    const onExit = () => {
      try {
        void getCurrentWindow().close();
      } catch {
        window.close();
      }
    };
    window.addEventListener("mieosql:exit", onExit);
    return () => window.removeEventListener("mieosql:exit", onExit);
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden bg-window text-slate-800">
      <TitleBar />
      <MenuBar />
      <Ribbon />
      <main className="flex min-h-0 flex-1">
        <Sidebar />
        {ready ? (
          <Workspace />
        ) : (
          <section className="grid flex-1 place-items-center bg-surface text-slate-400">
            Starting backend…
          </section>
        )}
        <RightPanel />
      </main>
      <StatusBar />
      <ConnectionModal />
      <FeatureDialogs />
      <Toasts />
    </div>
  );
}
