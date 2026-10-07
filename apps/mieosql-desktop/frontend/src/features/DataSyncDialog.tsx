// Placeholder — implemented by the Sync & Transfer feature agent.

import { DialogShell } from "./DialogShell";

export function DataSyncDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Data Synchronization" width={860} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Data sync is being implemented…
      </div>
    </DialogShell>
  );
}
