import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { OutboxItem, ScheduledSendReport } from "./correspondence";
import { adjacentOutboxKey, outboxEntries, selectedOutboxEntry } from "./outboxView";

/** Which Outbox row the reader shows. Like the conversation list, the first row is shown until another is chosen. */
export function useOutboxSelection(outbox: OutboxItem[], summaries: ScheduledSendReport[]) {
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const selectedRowRef = useRef<HTMLButtonElement | null>(null);
  const entries = useMemo(() => outboxEntries(outbox, summaries), [outbox, summaries]);
  const selectedEntry = selectedOutboxEntry(entries, selectedKey);
  const selectAdjacent = useCallback((direction: -1 | 1) => {
    setSelectedKey((current) => adjacentOutboxKey(entries, current, direction));
  }, [entries]);
  useEffect(() => {
    selectedRowRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [selectedEntry?.key]);
  return useMemo(
    () => ({ entries, selectedEntry, select: setSelectedKey, selectAdjacent, selectedRowRef }),
    [entries, selectedEntry, selectAdjacent],
  );
}
