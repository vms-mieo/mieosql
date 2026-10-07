// Placeholder — implemented by the Data Tools feature agent.

import { DialogShell } from "./DialogShell";

export function DataGenDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Data Generation" width={760} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Test-data generation is being implemented…
      </div>
    </DialogShell>
  );
}
