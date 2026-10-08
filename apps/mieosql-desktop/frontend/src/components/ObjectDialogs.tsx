// Host for the database/table management dialogs (new database/table, import,
// backup, restore). Controlled by `store.objectDialog`; App.tsx renders it.
//
// Other panels open one of these with:
//   useStore.getState().openObjectDialog("new-table", { sessionId, scope });

import type { Scope } from "../lib/bindings";
import { useStore } from "../lib/store";
import { BackupDialog } from "./dialogs/BackupDialog";
import { ImportDialog } from "./dialogs/ImportDialog";
import { NewDatabaseDialog } from "./dialogs/NewDatabaseDialog";
import { RestoreDialog } from "./dialogs/RestoreDialog";
import { TableDesignerDialog } from "./dialogs/TableDesignerDialog";

const EMPTY_SCOPE: Scope = {};

export function ObjectDialogs() {
  const dialog = useStore((s) => s.objectDialog);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const close = useStore((s) => s.closeObjectDialog);

  if (!dialog) return null;
  const { kind, props } = dialog;
  // Fall back to the active session when the opener did not pin one.
  const sessionId = props.sessionId ?? activeSessionId;
  const scope = props.scope ?? EMPTY_SCOPE;

  switch (kind) {
    case "new-database":
      return <NewDatabaseDialog sessionId={sessionId} onClose={close} />;
    case "new-table":
      return (
        <TableDesignerDialog
          mode="create"
          sessionId={sessionId}
          scope={scope}
          onClose={close}
        />
      );
    case "edit-table":
      return (
        <TableDesignerDialog
          mode="edit"
          sessionId={sessionId}
          scope={scope}
          table={props.table}
          onClose={close}
        />
      );
    case "import":
      return (
        <ImportDialog
          sessionId={sessionId}
          scope={scope}
          table={props.table}
          onClose={close}
        />
      );
    case "backup":
      // BackupDialog applies the session-database fallback itself.
      return (
        <BackupDialog sessionId={sessionId} scope={props.scope} onClose={close} />
      );
    case "restore":
      return <RestoreDialog sessionId={sessionId} onClose={close} />;
  }
}
