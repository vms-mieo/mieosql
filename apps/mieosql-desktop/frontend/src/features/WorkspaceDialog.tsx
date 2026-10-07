// Team Workspace: export / import a bundle of connection metadata. The bundle
// never contains passwords. F4 (Sync & Transfer) feature agent.

import { useState } from "react";
import { AlertTriangle, Download, Loader2, ShieldCheck, Upload } from "lucide-react";
import { open, save } from "@tauri-apps/plugin-dialog";

import { api, errorText } from "../lib/api";
import { useStore } from "../lib/store";
import type { ImportSummary, WorkspaceBundle } from "../lib/bindings";
import { DialogShell } from "./DialogShell";

const BTN_SECONDARY =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 hover:bg-slate-50 disabled:opacity-50";
const BTN_PRIMARY =
  "flex h-7 items-center gap-1.5 rounded-md bg-accent px-2.5 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50";

const BUNDLE_FILTER = [
  { name: "MieoSQL Workspace", extensions: ["json"] },
];

function stamp(): string {
  return new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-");
}

export function WorkspaceDialog({ onClose }: { onClose: () => void }) {
  const toast = useStore((s) => s.toast);
  const refreshProfiles = useStore((s) => s.refreshProfiles);

  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [overwrite, setOverwrite] = useState(false);
  const [bundle, setBundle] = useState<WorkspaceBundle | null>(null);
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  const onExport = async () => {
    setError(null);
    let path: string | null = null;
    try {
      path = await save({
        title: "Export Team Workspace",
        defaultPath: `mieosql-workspace-${stamp()}.json`,
        filters: BUNDLE_FILTER,
      });
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!path) return; // cancelled

    setBusy("export");
    try {
      const exported = await api.exportWorkspace(path);
      setBundle(exported);
      toast(
        "success",
        `Exported ${exported.profiles.length} connection(s) — share the bundle with your team (no passwords included)`,
      );
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  const onImport = async () => {
    setError(null);
    setSummary(null);
    let picked: string | null = null;
    try {
      const selection = await open({
        title: "Import Team Workspace",
        multiple: false,
        filters: BUNDLE_FILTER,
      });
      picked = typeof selection === "string" ? selection : null;
    } catch (err) {
      setError(errorText(err));
      return;
    }
    if (!picked) return; // cancelled

    setBusy("import");
    try {
      const result = await api.importWorkspace(picked, overwrite);
      setSummary(result);
      await refreshProfiles();
      if (result.imported === 0 && result.skipped > 0) {
        toast(
          "info",
          `No connections imported — ${result.skipped} already exist (enable overwrite to replace them)`,
        );
      } else {
        toast(
          "success",
          `Imported ${result.imported} connection(s)${
            result.skipped > 0 ? `, skipped ${result.skipped}` : ""
          }. Re-enter passwords for the imported connections.`,
        );
      }
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };

  return (
    <DialogShell title="Team Workspace" width={640} onClose={onClose}>
      <div className="space-y-3 p-3">
        <div className="flex items-start gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-600">
          <ShieldCheck size={14} className="mt-px shrink-0 text-slate-500" />
          <div>
            Workspace bundles contain connection <b>metadata only</b> — name,
            engine, host, port, database and SSL mode. Passwords and SSH
            secrets are never exported. After importing, re-enter the password
            for each connection before using it.
          </div>
        </div>

        <div className="rounded-md border border-slate-200 bg-white p-3">
          <div className="mb-1 text-[13px] font-semibold text-slate-900">
            Export
          </div>
          <p className="mb-2 text-[12.5px] text-slate-500">
            Write every saved connection to a JSON bundle to share with your
            teammates (for example over a repository or chat).
          </p>
          <button
            className={BTN_PRIMARY}
            onClick={() => void onExport()}
            disabled={busy !== null}
          >
            {busy === "export" ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Download size={12} />
            )}
            Export JSON…
          </button>
          {bundle && (
            <div className="mt-2 text-[11px] text-emerald-700">
              Last export: {bundle.profiles.length} connection(s) ·{" "}
              {bundle.exported_at}
            </div>
          )}
        </div>

        <div className="rounded-md border border-slate-200 bg-white p-3">
          <div className="mb-1 text-[13px] font-semibold text-slate-900">
            Import
          </div>
          <p className="mb-2 text-[12.5px] text-slate-500">
            Load a bundle exported from another MieoSQL workspace. Existing
            connections keep their stored passwords.
          </p>
          <label className="mb-2 flex items-center gap-1.5 text-[12.5px] text-slate-600">
            <input
              type="checkbox"
              checked={overwrite}
              onChange={(e) => setOverwrite(e.target.checked)}
            />
            Overwrite connections when the ID already exists
          </label>
          <button
            className={BTN_SECONDARY}
            onClick={() => void onImport()}
            disabled={busy !== null}
          >
            {busy === "import" ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Upload size={12} />
            )}
            Import JSON…
          </button>
        </div>

        {error && (
          <div className="flex items-start gap-2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-[12.5px] whitespace-pre-wrap text-rose-600">
            <AlertTriangle size={13} className="mt-px shrink-0" />
            <span>{error}</span>
          </div>
        )}

        {summary && (
          <div className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12.5px] text-slate-700">
            <div className="font-medium">
              Imported {summary.imported} · skipped {summary.skipped}
            </div>
            {summary.names.length > 0 && (
              <>
                <div className="mt-1 text-[11px] font-semibold tracking-[0.06em] text-slate-500 uppercase">
                  Collided connections ({overwrite ? "replaced" : "skipped"})
                </div>
                <ul className="custom-scrollbar mt-0.5 max-h-28 list-disc overflow-auto pl-4">
                  {summary.names.map((name, i) => (
                    <li key={`${name}-${i}`} className="font-code">
                      {name}
                    </li>
                  ))}
                </ul>
              </>
            )}
            <div className="mt-1.5 text-[11px] text-amber-700">
              Remember to re-enter passwords for the imported connections.
            </div>
          </div>
        )}
      </div>
    </DialogShell>
  );
}
