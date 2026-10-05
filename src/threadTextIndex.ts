import type { Message } from "./domain";
import { htmlToPlainText } from "./htmlPlainText";
import { buildThreadTextIndex, type ThreadTextIndex } from "./quotedHistory";
import { decodeHtmlEntities } from "./SafeMessage";

/**
 * Indexes a conversation's messages, oldest first, so each message's quote
 * folding can recognize text repeated from the messages before it. A message
 * without a plain-text part is indexed from its HTML, flattened in an inert
 * document.
 */
export function threadTextIndex(messages: readonly Pick<Message, "bodyText" | "bodyHtml">[]): ThreadTextIndex {
  return buildThreadTextIndex(messages.map(({ bodyText, bodyHtml }) => (bodyText.trim()
    ? decodeHtmlEntities(bodyText)
    : htmlToPlainText(bodyHtml, { whitespace: "collapse" }))));
}
