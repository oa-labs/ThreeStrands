import type { Thread } from "./domain";

const timeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
});

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "2-digit",
});

export function formatMailTimestamp(iso: string, now = new Date()): string {
  const date = new Date(iso);
  const isToday =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  return isToday ? timeFormatter.format(date) : dateFormatter.format(date);
}

export function formatTimeOnly(iso: string): string {
  return timeFormatter.format(new Date(iso));
}

export function formatAttachmentSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.ceil(size / 1024)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

const attachmentExtensionPattern = /\.[a-z0-9]{1,5}$/i;

/**
 * Splits a filename so the extension can stay visible while the base name
 * truncates. Returns no extension for dotfiles (".gitignore"), trailing dots
 * ("invoice."), and implausible suffixes ("itinerary.backup-2026-09").
 */
export function splitAttachmentName(filename: string): { base: string; extension: string } {
  const dot = filename.lastIndexOf(".");
  if (dot <= 0) return { base: filename, extension: "" };
  const extension = filename.slice(dot);
  if (!attachmentExtensionPattern.test(extension)) return { base: filename, extension: "" };
  return { base: filename.slice(0, dot), extension };
}

/**
 * Whether the thread has a message newer than its summary covers. Compares
 * against the revision the summary was written from; summaries saved before
 * that was recorded fall back to their generation time.
 */
export function isSummaryStale(thread: Pick<Thread, "summary" | "lastMessageAt" | "summaryRevision" | "summaryGeneratedAt">): boolean {
  if (!thread.summary) return false;
  const coveredUpTo = thread.summaryRevision ?? thread.summaryGeneratedAt;
  return Boolean(coveredUpTo && thread.lastMessageAt > coveredUpTo);
}

export function sortByRecency(threads: Thread[]): Thread[] {
  return [...threads].sort((a, b) => b.lastReceivedAt.localeCompare(a.lastReceivedAt));
}

export function triageNow(): number {
  return typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();
}
