import type { Draft } from "./correspondence";

export type MessageQuote = { messageId: string; text: string };

/** Read only a selection wholly inside the currently focused message body. */
export function selectedMessageQuote(): MessageQuote | null {
  const focused = document.activeElement;
  const frame = focused instanceof HTMLIFrameElement && focused.matches("iframe.message-body") ? focused : null;
  const selection = frame
    ? frame.contentWindow?.getSelection()
    : window.getSelection();
  if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
  const range = selection.getRangeAt(0);
  const body = frame?.contentDocument?.body
    ?? (range.commonAncestorContainer instanceof Element
      ? range.commonAncestorContainer.closest<HTMLElement>(".message-body-plain")
      : range.commonAncestorContainer.parentElement?.closest<HTMLElement>(".message-body-plain"));
  const article = (frame ?? body)?.closest<HTMLElement>(".message-card-expanded");
  if (!body || !article) return null;
  if (!body.contains(range.startContainer) || !body.contains(range.endContainer)) return null;
  const text = selection.toString().trim();
  const messageId = article.dataset.messageId;
  return text && messageId ? { messageId, text } : null;
}

/** Keep the source metadata and reply header supplied by the mail provider. */
export function draftWithSelectedQuote(draft: Draft, selectedText: string): Draft | null {
  if (draft.revision !== 0 || draft.bodyHtml?.trim()) return null;
  const header = draft.body.match(/^(\n\nOn [^\n]* wrote:\n)/)?.[1];
  const text = selectedText.trim();
  if (!header || !text) return null;
  return {
    ...draft,
    body: header + text.split(/\r\n|\r|\n/).map((line) => `> ${line}`).join("\n"),
  };
}
