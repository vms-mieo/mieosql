// Kafka Messages panel (feature tab "kafka-messages"): kafka-ui style
// message browser for one topic. Partition selector (with log ranges),
// position (Latest / Oldest / Offset), page size, one-shot fetch and a
// live tail that polls for new records. Opened from the broker tree
// (double-click / View Messages) via `store.openFeatureTab`.

import { useCallback, useEffect, useRef, useState } from "react";
import { Copy, Loader2, Play, RefreshCw, X } from "lucide-react";

import type { KafkaConsumeFrom, KafkaTopicPartition, Value } from "../lib/bindings";
import { api, errorText } from "../lib/api";
import { copyText } from "../lib/format";
import { useStore, type Tab } from "../lib/store";
import { DialogShell } from "./DialogShell";

const LIMIT_OPTIONS = [50, 100, 200, 500, 1000];
const LIVE_POLL_MS = 2000;
const MAX_LIVE_ROWS = 2000;

type Position = "latest" | "oldest" | "offset";

/** UI "oldest" maps to the backend "earliest" position. */
function toBackendFrom(
  position: Position,
  offset: string,
): KafkaConsumeFrom {
  if (position === "offset") {
    return { offset: /^\d+$/.test(offset.trim()) ? Number(offset.trim()) : 0 };
  }
  return position === "oldest" ? "earliest" : position;
}

type MsgRow = {
  partition: number;
  offset: number;
  ts: string;
  key: string;
  value: string;
  /** True when value parses as JSON (rendered in the accent color). */
  isJson: boolean;
};

function valText(v: Value): string {
  switch (v.t) {
    case "null":
      return "NULL";
    case "bool":
      return v.v ? "true" : "false";
    case "int":
    case "uint":
    case "decimal":
      return v.v;
    case "float":
      return String(v.v);
    case "text":
    case "date":
    case "time":
    case "datetime":
    case "uuid":
      return v.v;
    case "bytes":
      return v.v;
    case "json":
      return JSON.stringify(v.v);
    case "array":
      return v.v.map(valText).join(", ");
    case "unsupported":
      return `(unsupported ${v.v.native_type})`;
  }
}

function fmtTime(v: Value | undefined): string {
  if (!v) return "—";
  const text = valText(v);
  const d = new Date(text);
  return Number.isNaN(d.getTime()) ? text : d.toLocaleString();
}

function isJsonText(text: string): boolean {
  const t = text.trim();
  if (t.length < 2) return false;
  if ((t.startsWith("{") && t.endsWith("}")) || (t.startsWith("[") && t.endsWith("]"))) {
    try {
      JSON.parse(t);
      return true;
    } catch {
      return false;
    }
  }
  return false;
}

function prettyValue(text: string): { pretty: string; isJson: boolean } {
  try {
    return { pretty: JSON.stringify(JSON.parse(text), null, 2), isJson: true };
  } catch {
    return { pretty: text, isJson: false };
  }
}

const inputCls =
  "h-7 rounded-md border border-slate-300 bg-white px-2 text-[13px] text-slate-800 focus:ring-2 focus:ring-accent/40 focus:outline-none disabled:opacity-50";
const btnSecondary =
  "flex h-7 items-center gap-1.5 rounded-md border border-slate-300 bg-white px-2.5 text-[13px] text-slate-700 transition-colors hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-50";

export function KafkaMessagesPanel({ tab }: { tab: Tab }) {
  const props = (tab.featureProps ?? {}) as {
    sessionId?: string;
    topic?: string;
  };
  const sessionId = props.sessionId ?? null;
  const topic = props.topic ?? "";
  const toast = useStore((s) => s.toast);

  const [partitions, setPartitions] = useState<KafkaTopicPartition[]>([]);
  const [partition, setPartition] = useState<number | null>(null);
  const [position, setPosition] = useState<Position>("oldest");
  const [offset, setOffset] = useState("");
  const [limit, setLimit] = useState(100);
  const [rows, setRows] = useState<MsgRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [live, setLive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const [detail, setDetail] = useState<MsgRow | null>(null);
  const [partsLoading, setPartsLoading] = useState(false);

  // Next offset for live-tail polls (max seen + 1). A ref keeps the
  // interval callback fresh without re-subscribing every batch.
  const nextOffsetRef = useRef<number | null>(null);
  const liveStateRef = useRef({ sessionId, topic, partition, limit });
  liveStateRef.current = { sessionId, topic, partition, limit };

  const loadPartitions = useCallback(async () => {
    if (!sessionId || !topic) return;
    setPartsLoading(true);
    try {
      const parts = await api.kafkaTopicPartitions(sessionId, topic);
      setPartitions(parts);
      setPartition((p) =>
        p !== null && parts.some((x) => x.partition === p)
          ? p
          : (parts[0]?.partition ?? null),
      );
    } catch (err) {
      toast("error", errorText(err));
    } finally {
      setPartsLoading(false);
    }
  }, [sessionId, topic, toast]);

  useEffect(() => {
    void loadPartitions();
  }, [loadPartitions]);

  const fetchBatch = useCallback(
    async (opts: {
      from: "earliest" | "latest" | { offset: number };
      append: boolean;
    }): Promise<boolean> => {
      const { sessionId: sid, topic: t, partition: p, limit: lim } = liveStateRef.current;
      if (!sid || !t || p === null) return false;
      setLoading(true);
      setError(null);
      const started = Date.now();
      try {
        const out = await api.kafkaConsume(sid, t, p, opts.from, lim);
        const idx = (name: string) =>
          out.columns.findIndex((c) => c.name === name);
        const iPart = idx("partition");
        const iOff = idx("offset");
        const iTs = idx("timestamp");
        const iKey = idx("key");
        const iVal = idx("value");
        const batch: MsgRow[] = out.rows.map((r) => {
          const value = iVal >= 0 && r[iVal] ? valText(r[iVal]!) : "";
          return {
            partition: iPart >= 0 && r[iPart] ? Number(valText(r[iPart]!)) : p,
            offset: iOff >= 0 && r[iOff] ? Number(valText(r[iOff]!)) : 0,
            ts: iTs >= 0 && r[iTs] ? fmtTime(r[iTs]) : "—",
            key: iKey >= 0 && r[iKey] ? valText(r[iKey]!) : "",
            value,
            isJson: isJsonText(value),
          };
        });
        setElapsedMs(Date.now() - started);
        if (batch.length > 0) {
          const max = batch.reduce((m, r) => Math.max(m, r.offset), -1);
          nextOffsetRef.current = max + 1;
        } else if (!opts.append) {
          nextOffsetRef.current = null;
        }
        setRows((prev) => {
          const next = opts.append ? [...prev, ...batch] : batch;
          return next.length > MAX_LIVE_ROWS
            ? next.slice(next.length - MAX_LIVE_ROWS)
            : next;
        });
        return true;
      } catch (err) {
        const msg = errorText(err);
        setError(msg);
        return false;
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  const fetchOnce = useCallback(() => {
    setLive(false);
    nextOffsetRef.current = null;
    void fetchBatch({ from: toBackendFrom(position, offset), append: false });
  }, [fetchBatch, offset, position]);

  // Auto-fetch once the partitions arrive (first load).
  const autoFetched = useRef(false);
  useEffect(() => {
    if (!autoFetched.current && partition !== null && sessionId && topic) {
      autoFetched.current = true;
      fetchOnce();
    }
  }, [partition, sessionId, topic, fetchOnce]);

  // Live tail: poll from the last seen offset + 1.
  useEffect(() => {
    if (!live) return;
    const id = window.setInterval(() => {
      const from =
        nextOffsetRef.current !== null
          ? { offset: nextOffsetRef.current }
          : toBackendFrom(position, offset);
      void fetchBatch({ from, append: true }).then((ok) => {
        if (!ok) setLive(false);
      });
    }, LIVE_POLL_MS);
    return () => window.clearInterval(id);
  }, [live, fetchBatch, position, offset]);

  const copyValue = (text: string) => {
    void copyText(text).then((ok) =>
      toast(ok ? "success" : "error", ok ? "Copied value" : "Copy failed"),
    );
  };

  if (!sessionId || !topic) {
    return (
      <div className="grid flex-1 place-items-center text-[13px] text-slate-500">
        Messages needs a session and a topic — open it from the broker tree.
      </div>
    );
  }

  const offsetValid = /^\d+$/.test(offset.trim());
  const detailPretty = detail ? prettyValue(detail.value) : null;

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-surface">
      {/* toolbar */}
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-hairline px-3 py-2">
        <select
          className={inputCls}
          value={partition ?? ""}
          disabled={partsLoading || partitions.length === 0}
          title="Partition (log range)"
          onChange={(e) =>
            setPartition(e.target.value === "" ? null : Number(e.target.value))
          }
        >
          {partitions.map((p) => (
            <option key={p.partition} value={p.partition}>
              p{p.partition} ({p.earliest}→{p.latest})
            </option>
          ))}
          {partitions.length === 0 && <option value="">—</option>}
        </select>
        <button
          className={btnSecondary}
          title="Reload partitions"
          onClick={() => void loadPartitions()}
          disabled={partsLoading}
        >
          <RefreshCw size={13} className={partsLoading ? "animate-spin" : ""} />
        </button>
        <div className="flex overflow-hidden rounded-md border border-slate-300">
          {(["latest", "oldest", "offset"] as Position[]).map((m) => (
            <button
              key={m}
              className={`h-7 px-2.5 text-[13px] capitalize ${
                position === m
                  ? "bg-accent font-medium text-white"
                  : "bg-white text-slate-600 hover:bg-slate-50"
              }`}
              onClick={() => setPosition(m)}
            >
              {m}
            </button>
          ))}
        </div>
        {position === "offset" && (
          <input
            className={`${inputCls} w-28`}
            value={offset}
            placeholder="offset"
            inputMode="numeric"
            onChange={(e) => setOffset(e.target.value)}
          />
        )}
        <select
          className={inputCls}
          value={limit}
          title="Page size"
          onChange={(e) => setLimit(Number(e.target.value))}
        >
          {LIMIT_OPTIONS.map((n) => (
            <option key={n} value={n}>
              {n}/page
            </option>
          ))}
        </select>
        <button
          className="flex h-7 items-center gap-1.5 rounded-md bg-accent px-3 text-[13px] font-medium text-white hover:bg-accent-hover disabled:opacity-50"
          disabled={loading || partition === null || (position === "offset" && !offsetValid)}
          onClick={fetchOnce}
        >
          {loading && !live ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <Play size={13} strokeWidth={1.8} />
          )}
          Fetch
        </button>
        <button
          className={`flex h-7 items-center gap-1.5 rounded-md border px-2.5 text-[13px] transition-colors ${
            live
              ? "border-emerald-500 bg-emerald-50 font-medium text-emerald-700"
              : "border-slate-300 bg-white text-slate-600 hover:bg-slate-50"
          }`}
          title={live ? "Stop live tail" : "Tail new records every 2s"}
          onClick={() => {
            if (!live && rows.length > 0) {
              // Continue from the end of what is on screen.
              const max = rows.reduce((m, r) => Math.max(m, r.offset), -1);
              nextOffsetRef.current = max + 1;
            }
            setLive((v) => !v);
          }}
        >
          <span
            className={`inline-block h-2 w-2 rounded-full ${live ? "animate-pulse bg-emerald-500" : "bg-slate-300"}`}
          />
          Live
        </button>
        <span className="ml-auto text-[12px] font-medium text-slate-600">
          {rows.length} messages
          {elapsedMs !== null ? ` · ${elapsedMs} ms` : ""}
          {live ? " · live" : ""}
        </span>
      </div>

      {error && (
        <div className="shrink-0 border-b border-rose-200 bg-rose-50 px-3 py-1.5 text-[12.5px] text-rose-700">
          {error}
        </div>
      )}

      {/* rows */}
      <div className="custom-scrollbar min-h-0 flex-1 overflow-auto">
        {rows.length === 0 && !loading ? (
          <div className="grid h-full place-items-center p-8 text-center">
            <div>
              <p className="text-[13px] font-medium text-slate-700">
                No messages at this position.
              </p>
              <p className="mx-auto mt-1 max-w-[420px] text-[12.5px] text-slate-600">
                “Latest” only returns records that arrive after you fetch —
                quiet topics show nothing. Switch to “Oldest” to read from the
                start of the log, or turn on “Live” to tail new records.
              </p>
              {position !== "oldest" && (
                <button
                  className={`${btnSecondary} mx-auto mt-3`}
                  onClick={() => {
                    setPosition("oldest");
                    setLive(false);
                    nextOffsetRef.current = null;
                    void fetchBatch({ from: "earliest", append: false });
                  }}
                >
                  Load from oldest
                </button>
              )}
            </div>
          </div>
        ) : (
          <table className="w-full border-collapse text-[12.5px] text-slate-800">
            <thead className="sticky top-0 bg-grid-head">
              <tr className="text-left text-[12px] font-medium text-slate-600">
                <th className="w-20 border-b border-grid-line px-3 py-1.5">offset</th>
                <th className="w-44 border-b border-grid-line px-3 py-1.5">timestamp</th>
                <th className="w-48 border-b border-grid-line px-3 py-1.5">key</th>
                <th className="border-b border-grid-line px-3 py-1.5">
                  value{rows.some((r) => r.isJson) ? <span className="ml-1 font-normal text-accent">· JSON</span> : null}
                </th>
              </tr>
            </thead>
            <tbody className="font-code">
              {rows.map((r) => (
                <tr
                  key={`${r.offset}`}
                  className="grid-row cursor-pointer"
                  onClick={() => setDetail(r)}
                  title="Click for full message"
                >
                  <td className="grid-cell text-slate-600">{r.offset}</td>
                  <td className="grid-cell whitespace-nowrap text-slate-600">{r.ts}</td>
                  <td className="grid-cell font-semibold text-slate-800">
                    {r.key === "" ? <span className="font-normal text-slate-400">—</span> : r.key === "NULL" ? <span className="font-normal text-slate-500">[NULL]</span> : r.key}
                  </td>
                  <td className={`grid-cell ${r.isJson ? "text-accent" : ""}`}>{r.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {/* message detail */}
      {detail && detailPretty && (
        <DialogShell
          title={`partition ${detail.partition} @ offset ${detail.offset}`}
          onClose={() => setDetail(null)}
        >
          <div className="space-y-3 p-4 text-[12.5px]">
            <div className="grid grid-cols-[110px_1fr] gap-y-1 text-slate-600">
              <span>timestamp</span>
              <span className="text-slate-800">{detail.ts}</span>
              <span>key</span>
              <span className="break-all font-code text-slate-800">
                {detail.key === "" ? "—" : detail.key}
              </span>
              <span>format</span>
              <span>{detailPretty.isJson ? "JSON" : "text"}</span>
            </div>
            <pre className="custom-scrollbar max-h-[50vh] overflow-auto rounded-md border border-slate-200 bg-slate-50 p-3 font-code text-[12px] leading-relaxed whitespace-pre-wrap text-slate-800">
              {detailPretty.pretty}
            </pre>
            <div className="flex justify-end gap-2">
              <button className={btnSecondary} onClick={() => copyValue(detail.value)}>
                <Copy size={13} /> Copy value
              </button>
              <button className={btnSecondary} onClick={() => setDetail(null)}>
                <X size={13} /> Close
              </button>
            </div>
          </div>
        </DialogShell>
      )}
    </div>
  );
}
