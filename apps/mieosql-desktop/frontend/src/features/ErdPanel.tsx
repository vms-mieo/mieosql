// Placeholder — implemented by the ERD & BI feature agent.
//
// Props: `tab.featureProps = { sessionId?: string }`.

import type { Tab } from "../lib/store";

export function ErdPanel({ tab }: { tab: Tab }) {
  void tab;
  return (
    <div className="grid flex-1 place-items-center bg-white text-[11px] text-slate-400">
      ER diagram is being implemented…
    </div>
  );
}
