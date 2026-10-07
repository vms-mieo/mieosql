// Placeholder — implemented by the Data Tools feature agent.

import { DialogShell } from "./DialogShell";

export function MaskExportDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Export with Data Masking" width={760} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Masked export is being implemented…
      </div>
    </DialogShell>
  );
}
