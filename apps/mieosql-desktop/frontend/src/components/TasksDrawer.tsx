import { useEffect, useState } from "react";
import { ListTodo, X } from "lucide-react";

import { useTasks } from "../lib/tasks";

const DOT: Record<string, string> = {
  running: "animate-pulse bg-accent",
  done: "bg-emerald-500",
  error: "bg-rose-500",
};

function formatElapsed(ms?: number): string {
  if (ms == null) return "";
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

export function TasksDrawer() {
  const tasks = useTasks((s) => s.tasks);
  const clearFinished = useTasks((s) => s.clearFinished);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const onToggle = () => setOpen((v) => !v);
    window.addEventListener("mieosql:toggle-tasks", onToggle);
    return () => window.removeEventListener("mieosql:toggle-tasks", onToggle);
  }, []);

  if (!open) return null;

  const finished = tasks.filter((t) => t.status !== "running").length;

  return (
    <aside className="fixed top-0 right-0 z-40 flex h-full w-80 flex-col border-l border-slate-300/60 bg-surface shadow-xl">
      <header className="flex items-center justify-between border-b border-slate-200/70 px-3 py-2">
        <span className="flex items-center gap-1.5 text-[12.5px] font-semibold text-slate-700">
          <ListTodo size={14} strokeWidth={1.8} className="text-slate-500" />
          Background Tasks
        </span>
        <button
          type="button"
          onClick={() => setOpen(false)}
          className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600"
          title="Close"
        >
          <X size={14} />
        </button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {tasks.length === 0 ? (
          <p className="px-2 py-6 text-center text-[12px] text-slate-400">
            No background tasks yet.
            <br />
            Exports, backups and syncs will show up here.
          </p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {tasks.map((t) => (
              <li
                key={t.id}
                className="rounded-md border border-slate-200/70 bg-window px-2.5 py-2"
              >
                <div className="flex items-center gap-2">
                  <span
                    className={`inline-block h-2 w-2 shrink-0 rounded-full ${DOT[t.status]}`}
                  />
                  <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-slate-700">
                    {t.title}
                  </span>
                  {t.elapsedMs != null && (
                    <span className="font-code shrink-0 text-[11px] text-slate-400">
                      {formatElapsed(t.elapsedMs)}
                    </span>
                  )}
                </div>
                {(t.detail || t.error) && (
                  <p
                    className={`mt-1 truncate text-[11.5px] ${
                      t.status === "error" ? "text-rose-600" : "text-slate-500"
                    }`}
                    title={t.error ?? t.detail}
                  >
                    {t.error ?? t.detail}
                  </p>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
      <footer className="border-t border-slate-200/70 p-2">
        <button
          type="button"
          disabled={finished === 0}
          onClick={clearFinished}
          className="w-full rounded-md px-2 py-1.5 text-[12px] text-slate-500 hover:bg-slate-100 disabled:opacity-40"
        >
          Clear finished ({finished})
        </button>
      </footer>
    </aside>
  );
}
