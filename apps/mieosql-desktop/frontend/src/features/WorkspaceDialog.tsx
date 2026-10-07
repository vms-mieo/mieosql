// Placeholder — implemented by the Sync & Transfer feature agent.

import { DialogShell } from "./DialogShell";

export function WorkspaceDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Team Workspace" width={620} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Workspace export/import is being implemented…
      </div>
    </DialogShell>
  );
}
