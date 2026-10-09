// Keyboard shortcut cheatsheet (Beekeeper-style ShortcutHints as a dialog).

import { Keyboard } from "lucide-react";

import { DialogShell } from "./DialogShell";

const GROUPS: Array<{ title: string; rows: Array<[string, string]> }> = [
  {
    title: "Query editor",
    rows: [
      ["Run selection, or whole query", "⌘/Ctrl + Enter"],
      ["Format SQL", "⇧⌥F"],
      ["Minify SQL", "⇧⌥M"],
      ["Rename symbol under cursor", "F2"],
      ["Accept completion", "Tab"],
    ],
  },
  {
    title: "Result grid",
    rows: [
      ["Find in grid", "Ctrl + F"],
      ["Apply pending edits", "⌘/Ctrl + Enter"],
      ["Mark row for deletion", "⌫"],
      ["Next / previous find match", "Enter / ⇧Enter"],
    ],
  },
];

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return (
    <DialogShell title="Keyboard shortcuts" width={520} onClose={onClose}>
      <div className="flex flex-col gap-4 p-4">
        {GROUPS.map((g) => (
          <div key={g.title}>
            <div className="mb-1.5 flex items-center gap-1.5 text-[12px] font-semibold tracking-wide text-slate-500 uppercase">
              <Keyboard size={12} />
              {g.title}
            </div>
            {g.rows.map(([action, keys]) => (
              <div
                key={action}
                className="flex items-center justify-between py-1 text-[13px]"
              >
                <span className="text-slate-700">{action}</span>
                <kbd className="font-code rounded-md bg-slate-900/[0.06] px-1.5 py-0.5 text-[11.5px] text-slate-600">
                  {keys}
                </kbd>
              </div>
            ))}
          </div>
        ))}
      </div>
    </DialogShell>
  );
}
