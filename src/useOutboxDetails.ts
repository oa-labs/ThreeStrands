import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { OutboxItem, OutboxSummary } from "./correspondence";
import { mailClient } from "./data/client";
import { logBackgroundFailure } from "./errors";

/** Keeps bodies only for sends the current reader needs. Status polls do not reload bodies. */
export function useOutboxDetails(summaries: OutboxSummary[], ids: string[]) {
  const [items, setItems] = useState<OutboxItem[]>([]);
  const [attempt, setAttempt] = useState(0);
  const retry = useCallback(() => setAttempt((value) => value + 1), []);
  const [error, setError] = useState("");
  const current = useRef(items); current.current = items;
  const wanted = useMemo(() => summaries.filter((item) => ids.includes(item.id)), [summaries, ids]);
  const key = JSON.stringify(wanted.map((item) => [item.id, item.draft.revision]));
  const prime = useCallback((item: OutboxItem) => {
    setItems((previous) => [item, ...previous.filter((old) => old.id !== item.id)]);
  }, []);
  useEffect(() => {
    let active = true;
    const requested: [string, number][] = JSON.parse(key);
    const retained = current.current.filter((item) => requested.some(([id, revision]) => item.id === id && item.draft.revision === revision));
    setItems((previous) => previous.length === retained.length && previous.every((item, i) => item === retained[i]) ? previous : retained);
    setError("");
    const missing = requested.filter(([id]) => !retained.some((item) => item.id === id));
    // Load sequentially so a long thread does not start a request per send at once.
    void (async () => {
      for (const [id, revision] of missing) {
        if (!active) return;
        try {
          const next = await mailClient.getOutboxItem(id);
          if (!active) return;
          if (next.id !== id || next.draft.revision !== revision) continue;
          setItems((previous) => [...previous.filter((item) => item.id !== id), next]);
        } catch (reason) {
          if (active) { setError("Could not load outgoing message."); logBackgroundFailure("Outgoing message")(reason); }
        }
      }
    })();
    return () => { active = false; };
  }, [key, attempt]);
  const details = useMemo(() => wanted.flatMap((summary) => {
    const item = items.find((item) => item.id === summary.id && item.draft.revision === summary.draft.revision);
    // Delivery, ownership, and Sent-copy status always follow the latest summary.
    return item ? [{ ...item, ...summary, draft: { ...item.draft, updatedAt: summary.draft.updatedAt } }] : [];
  }), [wanted, items]);
  return { details, prime, error, retry, loading: details.length < wanted.length };
}
