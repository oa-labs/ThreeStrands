import type { Thread } from "./domain";

export type MessageFilterKind = "unread" | "starred" | "important" | "noReply";

export const MESSAGE_FILTER_OPTIONS: { kind: MessageFilterKind; label: string; shortcutKey: string }[] = [
  { kind: "unread", label: "Unread", shortcutKey: "U" },
  { kind: "starred", label: "Starred", shortcutKey: "S" },
  { kind: "important", label: "Important", shortcutKey: "I" },
  { kind: "noReply", label: "No Reply", shortcutKey: "R" },
];

function matchesMessageFilter(thread: Thread, kind: MessageFilterKind): boolean {
  switch (kind) {
    case "unread":
      return thread.unread;
    case "starred":
      return thread.starred;
    case "important":
      return thread.labels.includes("IMPORTANT");
    // A thread only carries Gmail's SENT label once one of its messages was sent from this account.
    case "noReply":
      return !thread.labels.includes("SENT");
  }
}

/** Threads must match every active filter to stay visible; filters narrow the list rather than union it. */
export function filterThreadsByMessageFilters(
  threads: Thread[],
  activeFilters: ReadonlySet<MessageFilterKind>,
): Thread[] {
  if (activeFilters.size === 0) return threads;
  return threads.filter((thread) =>
    [...activeFilters].every((kind) => matchesMessageFilter(thread, kind)),
  );
}
