import { selectedMessageQuote, type MessageQuote } from "./selectedMessageQuote";

/**
 * The passage selected in any expanded message of the open conversation, with
 * the message it is in, or null. Read only: an HTML body is read through its
 * frame's own selection and nothing in the frame is changed. Call it when a
 * command starts, since opening a menu or dialog can clear the selection.
 */
export function snapshotMessageSelection(): MessageQuote | null {
  for (const article of document.querySelectorAll<HTMLElement>(".message-card-expanded")) {
    const id = article.dataset.messageId;
    const quote = id ? selectedMessageQuote(id) : null;
    if (quote) return quote;
  }
  return null;
}
