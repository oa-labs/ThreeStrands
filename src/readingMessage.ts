import type { Message } from "./domain";
import { parseAddress, splitAddressList } from "./emailAddress";

/**
 * Where, as a fraction of the reader's height from its top, the message being
 * read is judged: the last message whose top has scrolled above this line.
 */
export const READING_LINE = 0.3;

/**
 * The message the reader is reading, from where the messages sit in the
 * scrolled reader. Scrolled to the end, the last message counts even when it
 * is too short to reach the reading line. `messages` are in display order.
 */
export function messageAtReadingLine(
  reader: { top: number; height: number; atBottom: boolean },
  messages: { id: string; top: number }[],
): string | null {
  if (messages.length === 0) return null;
  if (reader.atBottom) return messages[messages.length - 1].id;
  const line = reader.top + reader.height * READING_LINE;
  let reading = messages[0].id;
  for (const message of messages) {
    if (message.top <= line) reading = message.id;
    else break;
  }
  return reading;
}

/**
 * The person a message is about, for the context panel: its sender, or for a
 * message the user sent, its first recipient who is not the user. Null when
 * the message names nobody else.
 */
export function messagePerson(message: Pick<Message, "sender" | "recipients">, own: Set<string>): string | null {
  const sender = parseAddress(message.sender).email.toLocaleLowerCase();
  if (sender.includes("@") && !own.has(sender)) return sender;
  for (const raw of message.recipients.flatMap(splitAddressList)) {
    const email = parseAddress(raw).email.toLocaleLowerCase();
    if (email.includes("@") && !own.has(email)) return email;
  }
  return null;
}
