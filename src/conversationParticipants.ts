import type { ThreadDetail } from "./domain";
import { parseAddress, splitAddressList } from "./emailAddress";

/**
 * The people on a conversation other than the user: each sender and To
 * recipient once, by lowercase address, with the name the mail gave them.
 * `own` holds the user's account addresses, lowercased.
 */
export function conversationParticipants(detail: ThreadDetail | null, own: ReadonlySet<string>): { email: string; name: string }[] {
  if (!detail) return [];
  const byEmail = new Map<string, string>();
  for (const message of detail.messages) {
    for (const raw of [message.sender, ...message.recipients.flatMap(splitAddressList)]) {
      const address = parseAddress(raw);
      if (address.email.includes("@") && !own.has(address.email.toLocaleLowerCase())) {
        byEmail.set(address.email.toLocaleLowerCase(), address.name);
      }
    }
  }
  return [...byEmail].map(([email, name]) => ({ email, name }));
}
