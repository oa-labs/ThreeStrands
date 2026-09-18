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

export function sortByRecency(threads: Thread[]): Thread[] {
  return [...threads].sort((a, b) => b.lastReceivedAt.localeCompare(a.lastReceivedAt));
}

export function triageNow(): number {
  return typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();
}
