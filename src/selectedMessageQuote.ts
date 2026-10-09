import type { Draft } from "./correspondence";
import { plainTextToHtml } from "./richText";

export type MessageQuote = { messageId: string; text: string };

/** Read only a selection wholly inside the focused or explicitly targeted message body. */
export function selectedMessageQuote(messageId?: string): MessageQuote | null {
  const focused = document.activeElement;
  const target = messageId === undefined ? null : Array.from(document.querySelectorAll<HTMLElement>(".message-card-expanded"))
    .find((article) => article.dataset.messageId === messageId);
  if (messageId !== undefined && !target) return null;
  const frame = target ? target.querySelector<HTMLIFrameElement>("iframe.message-body")
    : focused instanceof HTMLIFrameElement && focused.matches("iframe.message-body") ? focused : null;
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
  const sourceId = article.dataset.messageId;
  if (messageId !== undefined && sourceId !== messageId) return null;
  return text && sourceId ? { messageId: sourceId, text } : null;
}

/** Keep the source metadata and reply header supplied by the mail provider. */
export function draftWithSelectedQuote(draft: Draft, selectedText: string): Draft | null {
  if (draft.revision !== 0 || draft.bodyHtml?.trim()) return null;
  const text = selectedText.trim().replace(/\r\n|\r/g, "\n");
  if (!text) return null;
  const quoted = text.split(/\r\n|\r|\n/).map((line) => `> ${line}`).join("\n");
  if (draft.mode === "forward") {
    const source = draft.forwardedContent?.text ?? draft.body.replace(/^\n\n/, "");
    const header = source.match(/^(---------- Forwarded message ----------\n[\s\S]*?\n\n)/)?.[1];
    if (!header) return null;
    return {
      ...draft,
      body: "",
      // Selected sender text stays in the existing sandboxed forwarding path.
      // Escape it as text rather than copying any sender HTML or resource URLs.
      forwardedContent: {
        html: `<div>${plainTextToHtml(header)}</div><blockquote type="cite">${plainTextToHtml(text)}</blockquote>`,
        text: header + quoted,
      },
    };
  }
  if (draft.mode !== "reply" && draft.mode !== "replyAll") return null;
  const header = draft.body.match(/^(\n\nOn [^\n]* wrote:\n)/)?.[1];
  if (!header) return null;
  return {
    ...draft,
    body: header + quoted,
  };
}
