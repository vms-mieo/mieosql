import {
  Database,
  Eye,
  FileCode2,
  Info,
  Plug,
  RefreshCw,
  Table2,
  UserRound,
  Wrench,
} from "lucide-react";

import { useStore } from "../lib/store";
import type { ObjectKind } from "../lib/bindings";

type RibbonButtonProps = {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  disabled?: boolean;
  title?: string;
  onClick?: () => void;
  accent?: string;
};

function RibbonButton({
  icon,
  label,
  active,
  disabled,
  title,
  onClick,
  accent = "text-slate-500",
}: RibbonButtonProps) {
  return (
    <button
      className={`flex min-w-[54px] flex-col items-center justify-center rounded border px-2 py-1 transition ${
        active
          ? "border-slate-300 bg-white text-blue-600 shadow-sm"
          : disabled
            ? "cursor-not-allowed border-transparent text-slate-300"
            : "border-transparent text-slate-700 hover:border-slate-300 hover:bg-white hover:shadow-sm"
      }`}
      disabled={disabled}
      onClick={onClick}
      title={title ?? label}
    >
      <span className={disabled ? "text-slate-300" : accent}>{icon}</span>
      <span className="mt-0.5 text-[10px] font-medium whitespace-nowrap">
        {label}
      </span>
    </button>
  );
}

export function Ribbon() {
  const store = useStore();
  const activeTab = store.tabs.find((t) => t.id === store.activeTabId);
  const sessionId = activeTab?.sessionId ?? store.activeSessionId ?? null;
  const session = store.sessions.find((s) => s.id === sessionId);

  const openFolder = (kind: ObjectKind, label: string) => {
    const selected = store.selected;
    if (!session) {
      store.toast("error", "Connect to a database first");
      return;
    }
    if (selected && selected.sessionId === session.id) {
      store.openObjects(selected.sessionId, selected.scope, kind, label);
      return;
    }
    // No object selected: open the folder view for the session's default scope.
    store.openObjects(session.id, {}, kind, label);
  };

  return (
    <section className="flex h-[64px] shrink-0 items-center justify-between border-b border-[#cbd5e1] bg-[#f1f5f9] px-3">
      <div className="flex items-center gap-1.5 overflow-x-auto">
        <RibbonButton
          icon={<Plug size={20} strokeWidth={1.8} />}
          label="Connection"
          accent="text-slate-500"
          onClick={() => store.setConnectionModal(true)}
        />
        <RibbonButton
          icon={<FileCode2 size={20} strokeWidth={1.8} />}
          label="New Query"
          accent="text-sky-500"
          onClick={() => store.newQueryTab()}
        />
        <RibbonButton
          icon={
            <span className="grid h-6 w-6 grid-cols-3 gap-[2px] rounded bg-sky-500 p-[3px]">
              {Array.from({ length: 6 }).map((_, i) => (
                <span key={i} className="rounded-[1px] bg-white/90" />
              ))}
            </span>
          }
          label="Table"
          title="Browse tables of the selected database/schema"
          onClick={() => openFolder("table", "Tables")}
        />
        <RibbonButton
          icon={<Eye size={19} strokeWidth={1.8} />}
          label="View"
          accent="text-sky-500"
          title="Browse views of the selected database/schema"
          onClick={() => openFolder("view", "Views")}
        />
        <RibbonButton
          icon={<Database size={18} strokeWidth={1.8} />}
          label="Objects"
          accent="text-emerald-600"
          title="Open the object browser"
          onClick={() => {
            if (!session) {
              store.toast("error", "Connect to a database first");
              return;
            }
            store.openObjects(session.id, {}, "table", "Objects");
          }}
        />
        <RibbonButton
          icon={<RefreshCw size={18} strokeWidth={1.8} />}
          label="Refresh"
          accent="text-teal-600"
          title="Reload connections and object tree"
          onClick={() => {
            void store.refreshProfiles();
            window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
          }}
        />
        <div className="mx-1 h-8 w-px bg-slate-300" />
        <RibbonButton
          icon={<Table2 size={18} strokeWidth={1.8} />}
          label="Query List"
          accent="text-orange-500"
          title="Open query history"
          onClick={() => store.setRightTab("history")}
        />
        <RibbonButton
          icon={<Wrench size={18} strokeWidth={1.8} />}
          label="Tools"
          disabled
          title="Import/Export, Backup — roadmap P1"
        />
        <RibbonButton
          icon={<UserRound size={18} strokeWidth={1.8} />}
          label="Users"
          disabled
          title="User management — roadmap P2"
        />
      </div>

      <div className="flex shrink-0 items-center gap-3 pl-3">
        <button
          className="text-sky-600 hover:text-sky-700"
          title={`MieoSQL backend ${store.apiVersion}`}
          onClick={() =>
            store.toast("info", `MieoSQL backend ${store.apiVersion || "…"}`)
          }
        >
          <Info size={18} />
        </button>
        <div className="grid h-8 w-8 place-items-center rounded-full border border-slate-300 bg-slate-200 text-slate-500">
          <UserRound size={16} />
        </div>
      </div>
    </section>
  );
}
