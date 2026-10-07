// Placeholder — implemented by the Code Insight feature agent.

import { DialogShell } from "./DialogShell";

export function QueryBuilderDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Query Builder" width={720} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Visual query builder is being implemented…
      </div>
    </DialogShell>
  );
}
