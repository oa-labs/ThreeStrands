import type { OutboxItem, ScheduledSendReport } from "./correspondence";

/**
 * One row of the Outbox: a send this computer owns, or a read-only report of
 * a schedule another device owns. Peer reports never carry message content.
 */
export type OutboxEntry =
  | { key: string; kind: "local"; item: OutboxItem }
  | { key: string; kind: "peer"; report: ScheduledSendReport };

/** Outbox rows in display order: this computer's sends, then peer schedules. Canceled sends are hidden. */
export function outboxEntries(outbox: OutboxItem[], summaries: ScheduledSendReport[]): OutboxEntry[] {
  return [
    ...outbox
      .filter((item) => item.state !== "canceled")
      .map<OutboxEntry>((item) => ({ key: `local:${item.id}`, kind: "local", item })),
    ...summaries
      .filter((report) => report.state !== "canceled")
      .map<OutboxEntry>((report) => ({ key: `peer:${report.ownerInstallationId}:${report.operationId}`, kind: "peer", report })),
  ];
}

/** The selected row, or the first one when nothing (or a row that has left the Outbox) is selected. */
export function selectedOutboxEntry(entries: OutboxEntry[], key: string | null): OutboxEntry | null {
  return entries.find((entry) => entry.key === key) ?? entries[0] ?? null;
}

/** The row `direction` steps from the selection, stopping at either end. */
export function adjacentOutboxKey(entries: OutboxEntry[], key: string | null, direction: -1 | 1): string | null {
  if (entries.length === 0) return null;
  const current = entries.findIndex((entry) => entry.key === selectedOutboxEntry(entries, key)?.key);
  const next = Math.min(Math.max(current + direction, 0), entries.length - 1);
  return entries[next].key;
}

/** The provider message an Outbox entry's Edit as New copies: only a delivered send whose message id is known. */
export function outboxEditAsNewSource(entry: OutboxEntry | null): { messageId: string; account: string } | null {
  if (entry?.kind !== "local" || entry.item.state !== "sent" || !entry.item.providerId) return null;
  return { messageId: entry.item.providerId, account: entry.item.draft.account };
}

/** When the row's time column points: the scheduled time, or when the message was last saved. */
export function outboxEntryTime(entry: OutboxEntry): number {
  return entry.kind === "peer"
    ? entry.report.scheduledAt
    : entry.item.schedule?.report.scheduledAt ?? entry.item.draft.updatedAt;
}
