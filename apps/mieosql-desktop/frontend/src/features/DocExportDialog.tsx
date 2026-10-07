// Placeholder — implemented by the Data Tools feature agent.

import { DialogShell } from "./DialogShell";

export function DocExportDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Data Dictionary" width={640} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Documentation export is being implemented…
      </div>
    </DialogShell>
  );
}
