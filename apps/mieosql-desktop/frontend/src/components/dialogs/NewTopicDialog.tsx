// New Kafka topic dialog. Opened via
// `store.openObjectDialog("new-topic", { sessionId })`.

import { useState } from "react";
import { Inbox, Loader2, Play } from "lucide-react";

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

function parsePositiveInt(raw: string): number | null {
  if (!/^\d+$/.test(raw.trim())) return null;
  const n = Number(raw.trim());
  return Number.isSafeInteger(n) && n >= 1 ? n : null;
}

export function NewTopicDialog({
  sessionId,
  onClose,
}: {
  sessionId: string | null;
  onClose: () => void;
}) {
  const session = useStore(
    (s) => s.sessions.find((x) => x.id === sessionId) ?? null,
  );
  const toast = useStore((s) => s.toast);

  const [name, setName] = useState("");
  const [partitions, setPartitions] = useState("3");
  const [replication, setReplication] = useState("1");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isKafka = session?.engine === "kafka";
  const topic = name.trim();
  const topicValid =
    topic.length >= 1 &&
    topic.length <= 249 &&
    /^[A-Za-z0-9._-]+$/.test(topic);
  const partitionsNum = parsePositiveInt(partitions);
  const replicationNum = parsePositiveInt(replication);
  const canRun =
    isKafka &&
    topicValid &&
    partitionsNum !== null &&
    replicationNum !== null &&
    !running &&
    sessionId !== null;

  const run = async () => {
    if (!sessionId || !canRun || partitionsNum === null || replicationNum === null) {
      return;
    }
    setRunning(true);
    setError(null);
    try {
      await api.kafkaCreateTopic(sessionId, topic, partitionsNum, replicationNum);
      toast(
        "success",
        `Topic “${topic}” created (${partitionsNum} partitions, RF ${replicationNum})`,
      );
      window.dispatchEvent(new CustomEvent("mieosql:refresh-tree"));
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setRunning(false);
    }
  };

  return (
    <ObjectDialogShell
      title="New Topic"
      width={520}
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
              <Play size={13} strokeWidth={1.8} />
            )}
            Create Topic
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
          <DialogError text="Open a Kafka connection before creating a topic." />
        )}

        {session && !isKafka && (
          <DialogNotice>
            New Topic is only available for Kafka connections.
          </DialogNotice>
        )}

        <div>
          <label className={DLG_LABEL}>Topic name *</label>
          <input
            className={DLG_INPUT}
            value={name}
            placeholder="orders"
            autoFocus
            disabled={running}
            onChange={(e) => setName(e.target.value)}
          />
          {topic.length > 0 && !topicValid && (
            <p className="mt-1 text-[11.5px] text-rose-600">
              Letters, digits, “.”, “_” and “-” only (1–249 chars).
            </p>
          )}
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className={DLG_LABEL}>Partitions *</label>
            <input
              className={DLG_INPUT}
              value={partitions}
              inputMode="numeric"
              disabled={running}
              onChange={(e) => setPartitions(e.target.value)}
            />
          </div>
          <div>
            <label className={DLG_LABEL}>Replication factor *</label>
            <input
              className={DLG_INPUT}
              value={replication}
              inputMode="numeric"
              disabled={running}
              onChange={(e) => setReplication(e.target.value)}
            />
          </div>
        </div>
        {(partitions.trim() && partitionsNum === null) ||
        (replication.trim() && replicationNum === null) ? (
          <p className="-mt-1 text-[11.5px] text-rose-600">
            Partitions and replication factor must be integers ≥ 1 (replication
            cannot exceed the broker count).
          </p>
        ) : null}

        {error && <DialogError text={error} />}
      </div>
    </ObjectDialogShell>
  );
}
