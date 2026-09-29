import type { ThreadDetail } from "./domain";
import { mailClient } from "./data/client";
import { parseAddress } from "./emailAddress";

/**
 * The shortest stay on a conversation before a proactive brief starts, so
 * paging through the inbox with a zero mark-read delay does not call the
 * provider for every conversation passed on the way.
 */
export const MIN_PROACTIVE_DWELL_SECONDS = 3;

export function proactiveDwellMs(autoReadDelaySeconds: number): number {
  return Math.max(autoReadDelaySeconds, MIN_PROACTIVE_DWELL_SECONDS) * 1000;
}

/**
 * The newest sender outside the user's own accounts when the conversation
 * qualifies for a proactive brief, otherwise `null`. Mailing lists (any
 * message offering unsubscribe) and conversations with no one else in them
 * never qualify.
 */
export function proactiveBriefSender(detail: ThreadDetail, ownAddresses: ReadonlySet<string>): string | null {
  if (detail.messages.some((message) => (message.unsubscribe?.methods.length ?? 0) > 0)) return null;
  for (const message of [...detail.messages].reverse()) {
    const sender = parseAddress(message.sender).email.toLocaleLowerCase();
    if (sender.includes("@") && !ownAddresses.has(sender)) return sender;
  }
  return null;
}

/** Whether the user has sent mail to this address before. */
export async function hasEmailedBefore(email: string): Promise<boolean> {
  const address = email.toLocaleLowerCase();
  const contacts = await mailClient.listContactProfiles(address, 20);
  return contacts.some((contact) => contact.sentCount > 0
    && contact.addresses.some((candidate) => candidate.toLocaleLowerCase() === address));
}
