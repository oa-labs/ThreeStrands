import type { Label } from "./domain";

const CATEGORY_PREFIX = "CATEGORY_";

// These Gmail system labels describe individual messages or duplicate state
// that already has dedicated controls in the reader. A thread can legitimately
// contain both INBOX and SENT messages, but presenting both as conversation
// labels makes the conversation itself look as though it lives in two places.
const MESSAGE_STATE_LABELS = new Set(["SENT", "DRAFT", "UNREAD", "STARRED"]);

/** Gmail's system category labels (CATEGORY_UPDATES, CATEGORY_SOCIAL, ...) carry
 * their raw id as the display name. Drop the prefix so they read as "UPDATES",
 * "SOCIAL", etc. */
export function formatLabelName(label: Pick<Label, "name">): string {
  return label.name.startsWith(CATEGORY_PREFIX) ? label.name.slice(CATEGORY_PREFIX.length) : label.name;
}

function labelDisplayRank(id: string): number {
  if (id === "INBOX") return 0;
  if (id.startsWith(CATEGORY_PREFIX)) return 1;
  return 2;
}

/** Orders a thread's label ids for display: INBOX first, then Gmail's category
 * labels, then everything else, preserving relative order within each group. */
export function sortLabelIdsForDisplay(ids: string[]): string[] {
  return [...ids].sort((a, b) => labelDisplayRank(a) - labelDisplayRank(b));
}

/** Removes message-level Gmail state from the labels shown for a conversation. */
export function labelIdsForConversationDisplay(ids: string[]): string[] {
  return sortLabelIdsForDisplay(ids.filter((id) => !MESSAGE_STATE_LABELS.has(id)));
}
