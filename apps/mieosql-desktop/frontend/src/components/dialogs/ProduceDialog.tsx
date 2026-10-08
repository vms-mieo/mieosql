// Produce a Kafka message. Opened via
// `store.openObjectDialog("produce-message", { sessionId, table: topic })`
// (`table` carries the prefilled topic, mirroring the table dialogs).

import { useState } from "react";
import { Inbox, Loader2, Send } from "lucide-react";

import { api, errorText } from "../../lib/api";
import { ENGINE_META } from "../../lib/engines";
import { useStore } from "../../lib/store";
import {
  DLG_BTN,
  DLG_BTN_PRIMARY,
  DLG_INPUT,
  DLG_LABEL,
  DialogError,
  DialogNotice,
  ObjectDialogShell,
} from "./DialogChrome";

export function ProduceDialog({
  sessionId,
  topic: initialTopic,
  onClose,
}: {
  sessionId: string | null;
  topic?: string;
  onClose: () => void;
}) {
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  const [topic, setTopic] = useState(initialTopic ?? "");
  const [partition, setPartition] = useState("");
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isKafka = session?.engine === "kafka";
  const topicName = topic.trim();
  const partitionTrimmed = partition.trim();
  const partitionValid =
    partitionTrimmed === "" || /^\d+$/.test(partitionTrimmed);
  const canRun =
    isKafka &&
    topicName.length > 0 &&
    value.length > 0 &&
    partitionValid &&
    !running &&
    sessionId !== null;

  const run = async () => {
    if (!sessionId || !canRun) return;
    setRunning(true);
    setError(null);
    try {
      const res = await api.kafkaProduce(
        sessionId,
        topicName,
        partitionTrimmed === "" ? null : Number(partitionTrimmed),
        key.trim() === "" ? null : key,
        value,
      );
      toast(
        "success",
        `Produced to “${topicName}” partition ${res.partition} @ offset ${res.offset}`,
      );
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="Produce Message"
      width={560}
      busy={running}
      onClose={onClose}
      footer={
        <>
          <button className={DLG_BTN} onClick={onClose} disabled={running}>
            Cancel
          </button>
          <button
            className={DLG_BTN_PRIMARY}
            onClick={() => void run()}
            disabled={!canRun}
            title={!isKafka ? "Kafka sessions only" : undefined}
          >
            {running ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Send size={13} strokeWidth={1.8} />
            )}
            Produce
          </button>
        </>
      }
    >
      <div className="space-y-3 p-4">
        <div className="flex items-center gap-2 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-[12px] text-slate-600">
          <Inbox size={13} strokeWidth={1.7} className="shrink-0 text-slate-500" />
          {session ? (
            <>
              <span className="font-medium text-slate-800">{session.name}</span>
              <span>{ENGINE_META[session.engine].label}</span>
            </>
          ) : (
            <span>No active session.</span>
          )}
        </div>

        {!session && (
          <DialogError text="Open a Kafka connection before producing." />
        )}

        {session && !isKafka && (
          <DialogNotice>
            Produce Message is only available for Kafka connections.
          </DialogNotice>
        )}

        <div className="grid grid-cols-[1fr_140px] gap-3">
          <div>
            <label className={DLG_LABEL}>Topic *</label>
            <input
              className={DLG_INPUT}
              value={topic}
              placeholder="orders"
              autoFocus={initialTopic === undefined}
              disabled={running}
              onChange={(e) => setTopic(e.target.value)}
            />
          </div>
          <div>
            <label className={DLG_LABEL}>Partition</label>
            <input
              className={DLG_INPUT}
              value={partition}
              placeholder="auto"
              inputMode="numeric"
              disabled={running}
              onChange={(e) => setPartition(e.target.value)}
            />
          </div>
        </div>
        {!partitionValid && (
          <p className="-mt-1 text-[11.5px] text-rose-600">
            Partition must be an integer ≥ 0, or empty for automatic
            partitioning (keyed records use Kafka&apos;s default partitioner).
          </p>
        )}

        <div>
          <label className={DLG_LABEL}>Key (optional)</label>
          <input
            className={DLG_INPUT}
            value={key}
            placeholder="order-123"
            disabled={running}
            onChange={(e) => setKey(e.target.value)}
          />
        </div>

        <div>
          <label className={DLG_LABEL}>Value *</label>
          <textarea
            className={`${DLG_INPUT} min-h-[120px] py-2 font-code text-[12px]`}
            value={value}
            placeholder={'{ "order_id": 123, "total": 45.6 }'}
            autoFocus={initialTopic !== undefined}
            disabled={running}
            onChange={(e) => setValue(e.target.value)}
          />
        </div>

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
