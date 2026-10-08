import type { ObjectKind } from "../lib/bindings";
import { isSqlEngine } from "../lib/quoting";
import { runScriptFile } from "../lib/run-script";
import { useStore } from "../lib/store";
import { ThemeBinder } from "../lib/theme";
import { PopupMenu, type MenuItem } from "./PopupMenu";
import { SettingsDialog } from "./SettingsDialog";
import {
  IconBackup,
  IconConnection,
  IconNewQuery,
  IconObjects,
  IconRefresh,
  IconSearch,
  IconTable,
  IconTools,
  IconUsers,
  IconView,
} from "./RibbonIcons";

/**
 * Toolbar ribbon (58px): large, colourful icons with labels — the Navicat
 * look. Primary actions (Connection, New Query), object browsers (Tables,
 * Views, Objects) and tools; everything else lives behind "Tools".
 */
function RibbonButton({
  icon,
  label,
  title,
  active,
  disabled,
  onClick,
}: {
  icon: React.ReactNode;
  label: string;
  title?: string;
  active?: boolean;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      className={`mx-0.5 flex w-[74px] shrink-0 flex-col items-center justify-center gap-[3px] rounded-lg py-1.5 transition-colors ${
        active ? "bg-accent/10" : "hover:bg-slate-900/[0.05]"
      } disabled:pointer-events-none disabled:opacity-40`}
      title={title ?? label}
      disabled={disabled}
      onClick={onClick}
    >
      <span className="grid h-8 w-8 place-items-center">{icon}</span>
      <span
        className={`text-[11px] leading-none ${
          active ? "font-semibold text-accent" : "font-medium text-slate-700"
        }`}
      >
        {label}
      </span>
    </button>
  );
}

function RibbonDivider() {
  return <div className="mx-1.5 my-2.5 w-px shrink-0 self-stretch bg-slate-200" />;
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

  const objectsTabActive = (label: string) =>
    activeTab?.kind === "objects" && activeTab.title === label;

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
    { kind: "label", label: "Backup & scripts" },
    {
      kind: "item",
      label: "New Database…",
      disabled: !session || !isSqlEngine(session.engine) || session.engine === "sqlite",
      onClick: () => {
        if (session) store.openObjectDialog("new-database", { sessionId: session.id });
      },
    },
    {
      kind: "item",
      label: "Open SQL File…",
      disabled: !session,
      onClick: () => void runScriptFile(),
    },
    {
      kind: "item",
      label: "Restore from SQL File…",
      disabled: !session,
      onClick: () => {
        if (session) store.openObjectDialog("restore", { sessionId: session.id });
      },
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
      label: "Users & Roles…",
      disabled: !session,
      onClick: () => {
        if (session) {
          store.setActiveSession(session.id);
          store.openFeatureTab("users", "Users & Roles", { sessionId: session.id });
        }
      },
    },
    {
      kind: "item",
      label: "Settings…",
      onClick: () => store.setSettingsOpen(true),
    },
    {
      kind: "item",
      label: `About MieoSQL (${store.apiVersion || "…"})`,
      onClick: () =>
        store.toast("info", `MieoSQL backend ${store.apiVersion || "…"}`),
    },
  ];

  return (
    <>
      <ThemeBinder />
      <SettingsDialog />
    <section className="flex h-[58px] shrink-0 items-stretch gap-0 border-b border-slate-200/70 bg-window px-2 select-none">
      <RibbonButton
        icon={<IconConnection />}
        label="Connection"
        title="New connection"
        onClick={() => store.setConnectionModal(true)}
      />
      <RibbonButton
        icon={<IconNewQuery />}
        label="New Query"
        title="New query (⌘N)"
        onClick={() => store.newQueryTab()}
      />

      <RibbonDivider />

      <RibbonButton
        icon={<IconTable />}
        label="Tables"
        title="Browse tables"
        active={objectsTabActive("Tables")}
        onClick={() => openFolder("table", "Tables")}
      />
      <RibbonButton
        icon={<IconView />}
        label="Views"
        title="Browse views"
        active={objectsTabActive("Views")}
        onClick={() => openFolder("view", "Views")}
      />
      <RibbonButton
        icon={<IconObjects />}
        label="Objects"
        title="All objects in the session"
        disabled={!session}
        onClick={() => {
          if (session) store.openObjects(session.id, {}, "table", "Objects");
        }}
      />

      <RibbonButton
        icon={<IconUsers />}
        label="Users"
        title="Manage database users, roles and privileges"
        disabled={!session}
        active={activeTab?.kind === "feature" && activeTab.featureId === "users"}
        onClick={() => {
          if (session) {
            store.setActiveSession(session.id);
            store.openFeatureTab("users", "Users & Roles", { sessionId: session.id });
          }
        }}
      />

      <RibbonDivider />

      <RibbonButton
        icon={<IconRefresh />}
        label="Refresh"
        title="Reload connections and object tree"
        onClick={() => {
          void store.refreshProfiles();
          window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
        }}
      />
      <RibbonButton
        icon={<IconSearch />}
        label="Search"
        title="Find objects and columns across the database"
        onClick={() => store.openFeatureDialog("search")}
      />
      <RibbonButton
        icon={<IconBackup />}
        label="Backup"
        title="Backup database"
        disabled={!session}
        onClick={() => {
          if (session) store.openObjectDialog("backup", { sessionId: session.id, scope: {} });
        }}
      />

      <RibbonDivider />

      <PopupMenu
        items={overflowItems}
        trigger={({ open, toggle }) => (
          <RibbonButton
            icon={<IconTools />}
            label="Tools"
            title="More actions"
            active={open}
            onClick={toggle}
          />
        )}
      />
    </section>
    </>
  );
}
