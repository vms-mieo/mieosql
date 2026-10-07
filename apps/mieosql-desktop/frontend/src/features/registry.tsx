// Feature tab registry: maps a Tab's `featureId` to its panel component.
//
// Owned by the integration layer — feature agents only provide the panel
// components themselves.

import type { Tab } from "../lib/store";
import { BiPanel } from "./BiPanel";
import { ErdPanel } from "./ErdPanel";
import { ExplainPanel } from "./ExplainPanel";

export function FeatureHost({ tab }: { tab: Tab }) {
  switch (tab.featureId) {
    case "explain":
      return <ExplainPanel tab={tab} />;
    case "erd":
      return <ErdPanel tab={tab} />;
    case "bi":
      return <BiPanel tab={tab} />;
    default:
      return (
        <div className="grid flex-1 place-items-center bg-white text-[11px] text-slate-400">
          Unknown feature “{tab.featureId}”
        </div>
      );
  }
}
