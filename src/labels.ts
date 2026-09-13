import type { Label } from "./domain";

const CATEGORY_PREFIX = "CATEGORY_";

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
