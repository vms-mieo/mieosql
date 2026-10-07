// Placeholder — implemented by the Sync & Transfer feature agent.

import { DialogShell } from "./DialogShell";

export function TransferDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Data Transfer" width={760} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Data transfer is being implemented…
      </div>
    </DialogShell>
  );
}
