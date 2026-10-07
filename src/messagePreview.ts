import type { Message } from "./domain";
import { MESSAGE_PREVIEW_MAX_CHARS } from "./contactContext";
import { htmlToPlainText } from "./htmlPlainText";
import { collapseQuotedHistoryText, type PriorThreadText } from "./quotedHistory";
import { decodeHtmlEntities } from "./SafeMessage";

/**
 * Plain-text parts spell out a link's target after its text, as
 * `text<mailto:a@b.example>` or `text<https://…>`, and replies nest them once
 * per quoting client. An image becomes `[https://…]` or `[cid:…]`.
 */
const linkAnnotation = /<(?:mailto:|https?:\/\/|tel:)[^<>\s]*>/gi;
const imagePlaceholder = /\[(?:https?:\/\/|cid:)[^\][\s]*\]/gi;

/** Removes link annotations from the inside out, since quoting nests them. */
function stripLinkAnnotations(text: string): string {
  let current = text;
  for (;;) {
    const next = current.replace(linkAnnotation, "");
    if (next === current) return current.replace(imagePlaceholder, "");
    current = next;
  }
}

const headerField = /^(from|sent|date|to|cc|subject)\s*:/i;
const emailOrTimestamp = /(?:[\w.+-]+@[\w.-]+\.[a-z]{2,}|\b\d{1,2}:\d{2}\b)/i;

/**
 * Cuts at a forwarded or replied-to message's From/Sent/To/Subject block.
 * The reader view folds only from stronger evidence, but a one-line preview
 * loses nothing the reader can't open, and the block itself is never what the
 * message says.
 */
function beforeHeaderBlock(text: string): string {
  const lines = text.split(/\r?\n/);
  for (let index = 1; index < lines.length; index++) {
    if (!/^from\s*:/i.test(lines[index].trim())) continue;
    const block = lines.slice(index, index + 6).map((line) => line.trim()).filter((line) => headerField.test(line));
    const fields = new Set(block.map((line) => line.match(headerField)![1].toLowerCase()));
    if (fields.size >= 3 && emailOrTimestamp.test(block.join(" "))) {
      const before = lines.slice(0, index).join("\n");
      if (before.trim()) return before;
    }
  }
  return text;
}

/**
 * One line summarizing what a message itself says: its quoted history and
 * plain-text link and image annotations are left out, whitespace is collapsed,
 * and long text stops at MESSAGE_PREVIEW_MAX_CHARS. Pass the earlier messages'
 * text so a reply whose quote has no attribution line still loses it.
 */
export function messagePreview(message: Pick<Message, "bodyText" | "bodyHtml">, prior?: PriorThreadText): string {
  const text = message.bodyText.trim()
    ? decodeHtmlEntities(message.bodyText)
    : htmlToPlainText(message.bodyHtml, { whitespace: "collapse" });
  const own = beforeHeaderBlock(collapseQuotedHistoryText(text, prior) ?? text);
  const line = stripLinkAnnotations(own).replace(/\s+/g, " ").trim();
  return line.length > MESSAGE_PREVIEW_MAX_CHARS ? `${line.slice(0, MESSAGE_PREVIEW_MAX_CHARS).trimEnd()}…` : line;
}
