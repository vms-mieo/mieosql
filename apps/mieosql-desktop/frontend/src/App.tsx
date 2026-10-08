import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

import { api } from "./lib/api";
import { useStore } from "./lib/store";
import { ConnectionModal } from "./components/ConnectionModal";
import { MenuBar } from "./components/MenuBar";
import { ObjectDialogs } from "./components/ObjectDialogs";
import { PromptHost } from "./components/PromptHost";
import { Ribbon } from "./components/Ribbon";
import { RightPanel } from "./components/RightPanel";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { TitleBar } from "./components/TitleBar";
import { Toasts } from "./components/Toasts";
import { Workspace } from "./components/Workspace";
import { FeatureDialogs } from "./features/dialogs";

const SIDEBAR_WIDTH_KEY = "mieosql.sidebarWidth";
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 520;
const SIDEBAR_DEFAULT = 240;

export default function App() {
  const ready = useStore((s) => s.ready);
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const stored = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY));
    return Number.isFinite(stored) &&
      stored >= SIDEBAR_MIN &&
      stored <= SIDEBAR_MAX
      ? stored
      : SIDEBAR_DEFAULT;
  });

  useEffect(() => {
    localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth));
  }, [sidebarWidth]);

  // Drag the divider between the connection sidebar and the workspace.
  const startSidebarResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = sidebarWidth;
    const onMove = (ev: MouseEvent) => {
      setSidebarWidth(
        Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startWidth + ev.clientX - startX)),
      );
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

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
        <Sidebar width={sidebarWidth} />
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize connection panel"
          title="Drag to resize · double-click to reset"
          className="group relative z-10 -ml-px w-[5px] shrink-0 cursor-col-resize"
          onMouseDown={startSidebarResize}
          onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT)}
        >
          <div className="absolute inset-y-0 left-[2px] w-px bg-transparent transition-colors group-hover:bg-accent/60" />
        </div>
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
      <ObjectDialogs />
      <FeatureDialogs />
      <PromptHost />
      <Toasts />
    </div>
  );
}
