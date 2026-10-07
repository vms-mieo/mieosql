// Placeholder — implemented by the Sync & Transfer feature agent.

import { DialogShell } from "./DialogShell";

export function StructureSyncDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Structure Synchronization" width={860} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Structure sync is being implemented…
      </div>
    </DialogShell>
  );
}
