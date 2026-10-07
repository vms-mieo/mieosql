import { Plug } from "lucide-react";

import { useStore } from "../lib/store";

export function StatusBar() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const session = store.sessions.find(
    (s) => s.id === (activeTab?.sessionId ?? store.activeSessionId),
  );
  const serverInfo = session ? store.serverInfos[session.id] : undefined;

  const running = store.tabs.some((t) => t.running);
  const anyError = activeTab?.results.some((r) => r.status === "error");
  const activeResult = activeTab?.results[activeTab.activeResult];

  return (
    <footer className="flex h-6 shrink-0 items-center justify-between border-t border-slate-300/60 bg-sidebar px-3 text-[11px] text-slate-500 select-none">
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="flex shrink-0 items-center gap-1.5">
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              running
                ? "animate-pulse bg-accent"
                : anyError
                  ? "bg-rose-500"
                  : "bg-emerald-500"
            }`}
          />
          <span className="text-slate-700">
            {running ? "Running…" : anyError ? "Error" : "Ready"}
          </span>
        </span>
        <span className="text-slate-300">·</span>
        <span className="flex min-w-0 items-center gap-1">
          <Plug size={11} strokeWidth={1.6} className="shrink-0 text-slate-400" />
          <span className="truncate">
            {session ? session.name : "No connection"}
            {serverInfo
              ? ` (${serverInfo.engine} ${shortVersion(serverInfo.version)})`
              : ""}
            {session?.database ? ` · ${session.database}` : ""}
          </span>
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-2.5">
        {activeResult && (
          <span className="font-code text-slate-600">
            {activeResult.rows.length.toLocaleString()} rows
            {activeResult.elapsedMs !== null
              ? ` · ${(activeResult.elapsedMs / 1000).toFixed(3)}s`
              : ""}
          </span>
        )}
        <span className="flex items-center gap-1.5">
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              session?.in_transaction ? "bg-amber-500" : "bg-slate-300"
            }`}
          />
          {session?.in_transaction ? "Transaction open" : "Auto Commit: ON"}
        </span>
        <span className="text-slate-300">·</span>
        <span className="font-code">
          Ln {store.cursor.line}, Col {store.cursor.col}
        </span>
        <span className="text-slate-300">·</span>
        <span className="font-code">v{store.apiVersion || "…"}</span>
      </div>
    </footer>
  );
}

function shortVersion(version: string): string {
  const m = version.match(/(\d+)(\.\d+)?(\.\d+)?/);
  return m ? m[0] : version.slice(0, 18);
}
