import type { Message } from "./domain";
import { parseAddress } from "./emailAddress";

/** Whether `message` came from the account that owns its thread. */
export function sentByAccount(message: Message, account: string | null | undefined): boolean {
  if (!account) return false;
  return parseAddress(message.sender).email.toLocaleLowerCase() === account.toLocaleLowerCase();
}

/**
 * The message "Edit as New" copies: the active message when the account sent
 * it, otherwise the latest message the account sent in the thread.
 */
export function editAsNewSource(
  messages: Message[],
  account: string | null | undefined,
  activeMessageId?: string | null,
): Message | null {
  const active = messages.find((message) => message.id === activeMessageId);
  if (active && sentByAccount(active, account)) return active;
  return [...messages].reverse().find((message) => sentByAccount(message, account)) ?? null;
}
