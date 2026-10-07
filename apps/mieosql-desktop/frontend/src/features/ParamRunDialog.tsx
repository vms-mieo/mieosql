// Placeholder — implemented by the Code Insight feature agent.
//
// Reads `store.paramDialog` ({ tabId, sql }) and calls `runWithParams`.

import { DialogShell } from "./DialogShell";

export function ParamRunDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Query Parameters" width={520} onClose={onClose}>
      <div className="p-6 text-[11px] text-slate-400">
        Parameter input is being implemented…
      </div>
    </DialogShell>
  );
}
