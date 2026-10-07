// Host for all feature dialogs; controlled by `store.featureDialog`.
//
// Owned by the integration layer — feature agents only provide the dialog
// components themselves (each takes `{ onClose }`).

import { useStore } from "../lib/store";
import { DataGenDialog } from "./DataGenDialog";
import { DataSyncDialog } from "./DataSyncDialog";
import { DocExportDialog } from "./DocExportDialog";
import { MaskExportDialog } from "./MaskExportDialog";
import { ParamRunDialog } from "./ParamRunDialog";
import { QueryBuilderDialog } from "./QueryBuilderDialog";
import { SearchDialog } from "./SearchDialog";
import { StructureSyncDialog } from "./StructureSyncDialog";
import { TransferDialog } from "./TransferDialog";
import { WorkspaceDialog } from "./WorkspaceDialog";

export function FeatureDialogs() {
  const dialog = useStore((s) => s.featureDialog);
  const close = useStore((s) => s.closeFeatureDialog);
  if (!dialog) return null;
  switch (dialog) {
    case "search":
      return <SearchDialog onClose={close} />;
    case "datagen":
      return <DataGenDialog onClose={close} />;
    case "mask":
      return <MaskExportDialog onClose={close} />;
    case "docs":
      return <DocExportDialog onClose={close} />;
    case "structure-sync":
      return <StructureSyncDialog onClose={close} />;
    case "data-sync":
      return <DataSyncDialog onClose={close} />;
    case "transfer":
      return <TransferDialog onClose={close} />;
    case "workspace":
      return <WorkspaceDialog onClose={close} />;
    case "params":
      return <ParamRunDialog onClose={close} />;
    case "builder":
      return <QueryBuilderDialog onClose={close} />;
    default:
      return null;
  }
}
