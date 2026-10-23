import type { TodayMail } from "./domain";
import { parseAddress } from "./emailAddress";
import { BULK_CATEGORY_LABELS } from "./senderSignals";

/** Most unread conversations Today lists; mirrors `MAX_TODAY_MAIL` in `src-tauri/src/db/today.rs`. */
export const TODAY_MAIL_LIMIT = 200;

export type TodayMailGroup = "reply" | "people" | "updates";

/** An address that says no one reads replies to it, whoever sends from it. */
const NO_REPLY_ADDRESS = /^(?:no-?reply|do-?not-?reply)(?:[+._-][^@]*)?@/i;

/**
 * Sorts an unread conversation into Today's groups from what its newest
 * inbound message says about itself, never from who the sender is:
 *
 * - updates: list mail (List-Unsubscribe, or an Updates/Promotions category)
 *   or a no-reply address.
 * - reply: written to you directly by someone you have written to, here or
 *   before.
 * - people: everything else a person sent.
 */
export function todayMailGroup(item: TodayMail): TodayMailGroup {
  const categorized = item.thread.labels.some((label) => BULK_CATEGORY_LABELS.includes(label));
  const noReply = item.sender !== null && NO_REPLY_ADDRESS.test(parseAddress(item.sender).email);
  if (item.listUnsubscribe || categorized || noReply) return "updates";
  if (item.addressedToMe && item.corresponded) return "reply";
  return "people";
}

export function groupTodayMail(items: readonly TodayMail[]): Record<TodayMailGroup, TodayMail[]> {
  const groups: Record<TodayMailGroup, TodayMail[]> = { reply: [], people: [], updates: [] };
  for (const item of items) groups[todayMailGroup(item)].push(item);
  return groups;
}
