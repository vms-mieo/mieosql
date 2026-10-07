// Placeholder — implemented by the ERD & BI feature agent.
//
// No required props; the panel picks its own data source.

import type { Tab } from "../lib/store";

export function BiPanel({ tab }: { tab: Tab }) {
  void tab;
  return (
    <div className="grid flex-1 place-items-center bg-white text-[11px] text-slate-400">
      BI dashboard is being implemented…
    </div>
  );
}
