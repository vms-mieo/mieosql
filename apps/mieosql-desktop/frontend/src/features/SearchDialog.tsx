// Placeholder — implemented by the Search & Explain feature agent.

import { DialogShell } from "./DialogShell";

export function SearchDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Find in Database" width={760} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Database-wide search is being implemented…
      </div>
    </DialogShell>
  );
}
