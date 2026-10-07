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
    <footer className="flex h-[22px] shrink-0 items-center justify-between border-t border-[#cbd5e1] bg-[#f1f5f9] px-2.5 text-[10px] text-slate-500 select-none">
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1.5">
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              running
                ? "animate-pulse bg-sky-500"
                : anyError
                  ? "bg-rose-500"
                  : "bg-emerald-500"
            }`}
          />
          <span className="text-slate-700">
            {running ? "Running…" : anyError ? "Error" : "Ready"}
          </span>
        </span>
        <span>|</span>
        <span>
          Connection:{" "}
          <span className="font-medium text-sky-700">
            {session ? session.name : "—"}
          </span>
          {serverInfo ? ` (${serverInfo.engine} ${shortVersion(serverInfo.version)})` : ""}
        </span>
        {session?.database && (
          <>
            <span>|</span>
            <span>
              DB: <span className="text-slate-700">{session.database}</span>
            </span>
          </>
        )}
        <span>|</span>
        <span className="font-code">
          Ln {store.cursor.line}, Col {store.cursor.col}
        </span>
      </div>
      <div className="flex items-center gap-3">
        {activeResult && (
          <span className="font-code text-slate-600">
            {activeResult.rows.length.toLocaleString()} rows fetched
            {activeResult.elapsedMs !== null
              ? ` in ${(activeResult.elapsedMs / 1000).toFixed(3)}s`
              : ""}
          </span>
        )}
        <span>|</span>
        <span className="flex items-center gap-1">
          <span
            className={`inline-block h-1.5 w-1.5 rounded-full ${
              session?.in_transaction ? "bg-amber-500" : "bg-slate-300"
            }`}
          />
          {session?.in_transaction ? "Transaction: OPEN" : "Autocommit: ON"}
        </span>
        <span>|</span>
        <span className="font-code">v{store.apiVersion || "…"}</span>
      </div>
    </footer>
  );
}

function shortVersion(version: string): string {
  const m = version.match(/(\d+)(\.\d+)?(\.\d+)?/);
  return m ? m[0] : version.slice(0, 18);
}
