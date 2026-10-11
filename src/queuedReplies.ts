import type { Draft, OutboxItem } from "./correspondence";
import type { Message, ThreadDetail } from "./domain";
import { decodeHtmlEntities } from "./SafeMessage";
import { parseAddress } from "./emailAddress";
// A provider message can match a local draft only if it was sent no earlier
// than this before the draft's last save, allowing for clock skew.
const DRAFT_MATCH_CLOCK_SKEW_MS = 60_000;

function splitDraftRecipients(draft: Draft): string[] {
  const result: string[] = [];
  for (const field of [draft.to, draft.cc]) {
    let start = 0;
    let quoted = false;
    let angleDepth = 0;
    for (let index = 0; index <= field.length; index++) {
      const character = field[index];
      if (character === '"' && field[index - 1] !== "\\") quoted = !quoted;
      else if (!quoted && character === "<") angleDepth++;
      else if (!quoted && character === ">") angleDepth = Math.max(0, angleDepth - 1);
      if (index === field.length || (character === "," && !quoted && angleDepth === 0)) {
        const address = field.slice(start, index).trim();
        if (address) result.push(address);
        start = index + 1;
      }
    }
  }
  return result;
}

function comparableMessageBody(value: string): string {
  return decodeHtmlEntities(value).replace(/\s+/g, " ").trim().toLocaleLowerCase();
}

function providerMessageMatchesDraft(message: Message, draft: Draft): boolean {
  if (parseAddress(message.sender).email.toLocaleLowerCase() !== draft.account.toLocaleLowerCase()) return false;
  if (new Date(message.sentAt).getTime() < draft.updatedAt - DRAFT_MATCH_CLOCK_SKEW_MS) return false;
  const actual = comparableMessageBody(message.bodyText);
  const expected = comparableMessageBody(draft.body);
  return Boolean(expected) && (actual === expected || actual.includes(expected) || expected.includes(actual));
}

/**
 * Adds locally queued replies to their open conversation immediately. Once
 * Gmail's copy reaches the thread cache it wins, preventing a duplicate.
 */
export function messagesWithQueuedReplies(detail: ThreadDetail, outbox: OutboxItem[]): Message[] {
  const queuedReplies = outbox
    .filter((item) =>
      ["reply", "replyAll"].includes(item.draft.mode)
      && !["canceled", "failed", "scheduled", "overdue"].includes(item.state)
      && Boolean(item.draft.sourceId)
      && detail.messages.some((message) => message.id === item.draft.sourceId)
      && !detail.messages.some((message) =>
        (item.providerId && message.id === item.providerId)
        || providerMessageMatchesDraft(message, item.draft))
      )
    .sort((left, right) => left.draft.updatedAt - right.draft.updatedAt)
    .map<Message>((item) => ({
      id: `outbox-${item.id}`,
      threadId: detail.thread.id,
      sender: item.draft.account,
      recipients: splitDraftRecipients(item.draft),
      sentAt: new Date(item.draft.updatedAt).toISOString(),
      bodyHtml: item.draft.bodyHtml ?? "",
      bodyText: item.draft.body,
      unread: false,
      unsubscribe: null,
      attachments: item.draft.attachments.map((attachment) => ({
        id: attachment.id,
        filename: attachment.name,
        mimeType: attachment.mime,
        size: attachment.size,
        contentId: attachment.contentId,
        inline: attachment.inline,
      })),
    }));

  return queuedReplies.length > 0 ? [...detail.messages, ...queuedReplies] : detail.messages;
}
