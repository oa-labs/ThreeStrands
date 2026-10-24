import type { TodayMail } from "./domain";
import { parseAddress } from "./emailAddress";
import { BULK_CATEGORY_LABELS } from "./senderSignals";

/** Most unread conversations Today lists; mirrors `MAX_TODAY_MAIL` in `src-tauri/src/db/today.rs`. */
export const TODAY_MAIL_LIMIT = 200;

export type TodayMailGroup = "invitations" | "reply" | "people" | "updates";

/** Most addresses on the To line for mail to still count as written to you rather than to a group. */
export const DIRECT_TO_LIMIT = 3;

/** An address that says no one reads replies to it, whoever sends from it. */
const NO_REPLY_ADDRESS = /^(?:no-?reply|do-?not-?reply)(?:[+._-][^@]*)?@/i;

/**
 * Sorts an unread conversation into Today's groups from what its newest
 * inbound message says about itself, never from who the sender is:
 *
 * - invitations: it carries a calendar invitation, which asks for an answer
 *   rather than a written reply.
 * - updates: list mail (List-Unsubscribe, or an Updates/Promotions category)
 *   or a no-reply address.
 * - reply: written to you directly, with at most DIRECT_TO_LIMIT addresses on
 *   the To line, by someone you have written to, here or before.
 * - people: everything else a person sent.
 */
export function todayMailGroup(item: TodayMail): TodayMailGroup {
  const categorized = item.thread.labels.some((label) => BULK_CATEGORY_LABELS.includes(label));
  const noReply = item.sender !== null && NO_REPLY_ADDRESS.test(parseAddress(item.sender).email);
  if (item.hasInvitation) return "invitations";
  if (item.listUnsubscribe || categorized || noReply) return "updates";
  if (item.addressedToMe && item.toCount <= DIRECT_TO_LIMIT && item.corresponded) return "reply";
  return "people";
}

export function groupTodayMail(items: readonly TodayMail[]): Record<TodayMailGroup, TodayMail[]> {
  const groups: Record<TodayMailGroup, TodayMail[]> = { invitations: [], reply: [], people: [], updates: [] };
  for (const item of items) groups[todayMailGroup(item)].push(item);
  return groups;
}
