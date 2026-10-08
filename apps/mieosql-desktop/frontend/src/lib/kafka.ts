// Kafka UI helpers: consume-JSON builder plus the "view messages" flow,
// which opens the kafka-ui style Messages panel for a topic.

import { useStore } from "./store";

export type ConsumeFrom = "earliest" | "latest" | number;

/** Build the consume JSON command the backend Kafka driver executes. */
export function consumeJson(
  topic: string,
  partition: number,
  from: ConsumeFrom,
  limit: number,
): string {
  const fromPart = typeof from === "number" ? `${from}` : `"${from}"`;
  return `{ "topic": ${JSON.stringify(topic)}, "partition": ${partition}, "from": ${fromPart}, "limit": ${limit} }`;
}

/** Open the Messages panel for `topic` (partition/position picked there). */
export function openMessages(sessionId: string, topic: string): void {
  const store = useStore.getState();
  store.setActiveSession(sessionId);
  store.openFeatureTab("kafka-messages", `Messages · ${topic}`, {
    sessionId,
    topic,
  });
}
