// Add / edit index dialog for a table (opened from the Object inspector).

import { useState } from "react";

import { api, errorText } from "../../lib/api";
import type { ColumnDef, Engine, IndexDef, Scope } from "../../lib/bindings";
import {
  buildCreateIndex,
  buildDropIndex,
  suggestIndexName,
} from "../../lib/index-ddl";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_INPUT,
  DLG_LABEL,
  DialogError,
  ObjectDialogShell,
} from "./DialogChrome";

export function IndexDialog({
  sessionId,
  engine,
  scope,
  table,
  columns,
  editing,
  onClose,
  onApplied,
}: {
  sessionId: string;
  engine: Engine;
  scope: Scope;
  table: string;
  columns: ColumnDef[];
  /** Existing index to edit (drop + recreate); null = create. */
  editing: IndexDef | null;
  onClose: () => void;
  onApplied: () => void;
}) {
  const store = useStore();
  const [name, setName] = useState(
    () => editing?.name ?? suggestIndexName(table, []),
  );
  const [unique, setUnique] = useState(editing?.is_unique ?? false);
  const [picked, setPicked] = useState<Set<string>>(
    () => new Set(editing?.columns ?? []),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggle = (col: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(col)) next.delete(col);
      else next.add(col);
      return next;
    });

  const apply = async () => {
    const chosen = columns.filter((c) => picked.has(c.name)).map((c) => c.name);
    const sql = buildCreateIndex(engine, scope, table, {
      name,
      columns: chosen,
      unique,
    });
    if (!sql) {
      setError("Enter an index name and pick at least one column.");
      return;
    }
    const statements = editing
      ? [buildDropIndex(engine, scope, table, editing.name), sql].filter(
          (s): s is string => !!s,
        )
      : [sql];
    setBusy(true);
    setError(null);
    try {
      const result = await api.executeBatch(sessionId, statements, false);
      if (result.failed > 0) {
        setError(result.errors[0] ?? "Statement failed.");
        return;
      }
      store.toast(
        "success",
        editing ? `Index “${name}” updated` : `Index “${name}” created`,
      );
      onApplied();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ObjectDialogShell
      title={editing ? `Edit index · ${editing.name}` : `New index on ${table}`}
      width={460}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void apply()}
            disabled={busy || !name.trim() || picked.size === 0}
          >
            {busy ? "Applying…" : editing ? "Save index" : "Create index"}
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        {editing && (
          <p className="rounded-md bg-amber-50 px-2.5 py-2 text-[12px] leading-relaxed text-amber-700">
            Most engines cannot alter an index in place — saving drops
            <span className="font-code"> {editing.name} </span>
            and recreates it with the new definition.
          </p>
        )}

        <div>
          <label className={DLG_LABEL}>Name</label>
          <input
            className={DLG_INPUT}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="ix_table_column"
            autoFocus
          />
        </div>

        <label className="flex items-center gap-2 text-[13px] text-slate-700">
          <input
            type="checkbox"
            className="h-3.5 w-3.5 accent-accent"
            checked={unique}
            onChange={(e) => setUnique(e.target.checked)}
          />
          Unique index
        </label>

        <div>
          <label className={DLG_LABEL}>
            Columns ({picked.size} selected)
          </label>
          <div className="custom-scrollbar max-h-[240px] overflow-auto rounded-md border border-slate-200">
            {columns.length === 0 && (
              <div className="p-2.5 text-[12.5px] text-slate-500">
                Columns are still loading…
              </div>
            )}
            {columns.map((c) => (
              <label
                key={c.name}
                className="flex cursor-default items-center gap-2 border-b border-slate-100 px-2.5 py-1.5 last:border-b-0 hover:bg-slate-50"
              >
                <input
                  type="checkbox"
                  className="h-3.5 w-3.5 shrink-0 accent-accent"
                  checked={picked.has(c.name)}
                  onChange={() => toggle(c.name)}
                />
                <span className="min-w-0 flex-1 truncate font-code text-[12.5px] text-slate-800">
                  {c.name}
                </span>
                <span className="shrink-0 text-[11px] text-slate-400">
                  {c.native_type}
                </span>
              </label>
            ))}
          </div>
        </div>

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
