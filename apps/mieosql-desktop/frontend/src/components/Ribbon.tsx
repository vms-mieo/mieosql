import {
  FileCode2,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Search,
} from "lucide-react";

import { useStore } from "../lib/store";
import type { ObjectKind } from "../lib/bindings";
import { PopupMenu, type MenuItem } from "./PopupMenu";

function ToolbarButton({
  icon,
  label,
  title,
  disabled,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  title?: string;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      className="flex h-7 items-center gap-1.5 rounded-md px-2 text-[13px] font-medium text-slate-700 transition-colors hover:bg-slate-900/[0.05] disabled:pointer-events-none disabled:text-slate-300"
      disabled={disabled}
      onClick={onClick}
      title={title ?? label}
    >
      <span className={disabled ? "text-slate-300" : "text-slate-500"}>
        {icon}
      </span>
      {label}
    </button>
  );
}

/**
 * Slim toolbar (48px): primary + contextual actions only. Everything else
 * lives behind the "⋯" overflow (mirrors the Tools menu) — Apple-style
 * primary/contextual hierarchy instead of a grid of equal buttons.
 */
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

  const overflowItems: MenuItem[] = [
    { kind: "label", label: "Browse" },
    {
      kind: "item",
      label: "Tables",
      onClick: () => openFolder("table", "Tables"),
    },
    {
      kind: "item",
      label: "Views",
      onClick: () => openFolder("view", "Views"),
    },
    {
      kind: "item",
      label: "Objects",
      disabled: !session,
      onClick: () => {
        if (!session) return;
        store.openObjects(session.id, {}, "table", "Objects");
      },
    },
    { kind: "separator" },
    { kind: "label", label: "Query" },
    {
      kind: "item",
      label: "Query History",
      onClick: () => store.setRightTab("history"),
    },
    {
      kind: "item",
      label: "Find in Database…",
      shortcut: "⇧⌘F",
      onClick: () => store.openFeatureDialog("search"),
    },
    { kind: "separator" },
    { kind: "label", label: "Design & tools" },
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
      label: "Dashboard",
      onClick: () => store.openFeatureTab("bi", "Dashboard"),
    },
    {
      kind: "item",
      label: "Data Tools…",
      disabled: !session,
      onClick: () => store.openFeatureDialog("datagen"),
    },
    { kind: "separator" },
    {
      kind: "item",
      label: "Users — coming soon",
      disabled: true,
      onClick: () => {},
    },
    {
      kind: "item",
      label: `About MieoSQL (${store.apiVersion || "…"})`,
      onClick: () =>
        store.toast("info", `MieoSQL backend ${store.apiVersion || "…"}`),
    },
  ];

  return (
    <section className="flex h-12 shrink-0 items-center gap-1 bg-window px-3">
      <ToolbarButton
        icon={<Plus size={16} strokeWidth={1.8} />}
        label="Connection"
        title="New connection"
        onClick={() => store.setConnectionModal(true)}
      />
      <ToolbarButton
        icon={<FileCode2 size={16} strokeWidth={1.8} />}
        label="New Query"
        title="New query (⌘N)"
        onClick={() => store.newQueryTab()}
      />
      <div className="mx-1.5 h-5 w-px bg-slate-300/70" />
      <ToolbarButton
        icon={<RefreshCw size={15} strokeWidth={1.8} />}
        label="Refresh"
        title="Reload connections and object tree"
        onClick={() => {
          void store.refreshProfiles();
          window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
        }}
      />
      <ToolbarButton
        icon={<Search size={15} strokeWidth={1.8} />}
        label="Search"
        title="Find objects and columns across the database"
        onClick={() => store.openFeatureDialog("search")}
      />
      <PopupMenu
        items={overflowItems}
        trigger={({ open, toggle }) => (
          <button
            className={`grid h-7 w-7 place-items-center rounded-md transition-colors ${
              open
                ? "bg-slate-900/[0.07] text-slate-700"
                : "text-slate-500 hover:bg-slate-900/[0.05] hover:text-slate-700"
            }`}
            title="More actions"
            onClick={toggle}
          >
            <MoreHorizontal size={17} strokeWidth={1.8} />
          </button>
        )}
      />
    </section>
  );
}
