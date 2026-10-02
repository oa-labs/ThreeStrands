import type { Message } from "./domain";

/** An attachment in the open conversation that Thread Chat can read. */
export type ChatAttachmentOption = {
  messageId: string;
  attachmentId: string;
  filename: string;
  sender: string;
  sentAt: string;
};

/** Matches `attachment_text::MAX_CHAT_ATTACHMENTS` in the native app. */
export const MAX_CHAT_ATTACHMENTS = 4;
/** Most matches the @ menu lists at once. */
export const MAX_ATTACHMENT_MATCHES = 8;
/** Longest text after @ still treated as a filename being typed. */
const MAX_MENTION_QUERY_CHARS = 80;

const READABLE_EXTENSIONS = new Set(["txt", "text", "csv", "tsv", "md", "markdown", "log", "pdf", "docx", "xlsx", "pptx"]);
const READABLE_TYPES = new Set([
  "text/plain",
  "text/csv",
  "text/tab-separated-values",
  "text/markdown",
  "application/pdf",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
]);

/**
 * Whether chat can read the file: text, PDF, Word, Excel, or PowerPoint.
 * Mirrors `attachment_text::kind_for`, which remains the authority.
 */
export function isChatReadableAttachment(filename: string, mimeType: string): boolean {
  const dot = filename.lastIndexOf(".");
  if (dot >= 0) {
    const extension = filename.slice(dot + 1).toLowerCase();
    if (READABLE_EXTENSIONS.has(extension)) return true;
  }
  return READABLE_TYPES.has(mimeType.split(";")[0].trim().toLowerCase());
}

/** Readable, non-inline attachments in the conversation, newest message first. */
export function chatAttachmentOptions(messages: Message[]): ChatAttachmentOption[] {
  return [...messages].reverse().flatMap((message) => message.attachments
    .filter((attachment) => !attachment.inline && isChatReadableAttachment(attachment.filename, attachment.mimeType))
    .map((attachment) => ({
      messageId: message.id,
      attachmentId: attachment.id,
      filename: attachment.filename,
      sender: message.sender,
      sentAt: message.sentAt,
    })));
}

export const attachmentKey = (attachment: { messageId: string; attachmentId: string }) =>
  `${attachment.messageId}\u0000${attachment.attachmentId}`;

/** The @ being typed at the caret: where it starts and the text after it. */
export type Mention = { start: number; query: string };

/**
 * The mention the caret is in, if any. An @ counts only at the start or
 * after whitespace, so addresses like sam@example.com never open the menu.
 */
export function activeMention(text: string, caret: number): Mention | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("@");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1])) return null;
  const query = before.slice(start + 1);
  if (query.includes("\n") || query.length > MAX_MENTION_QUERY_CHARS) return null;
  return { start, query };
}

/**
 * Attachments whose filename contains the query, prefix matches first. A
 * mention already completed as `@filename ` matches nothing, so the menu
 * doesn't reopen behind the caret.
 */
export function matchAttachments(options: ChatAttachmentOption[], query: string): ChatAttachmentOption[] {
  const needle = query.toLowerCase();
  if (/\s$/.test(query) && options.some((option) => option.filename.toLowerCase() === needle.trimEnd())) return [];
  const scored = options
    .map((option, index) => ({ option, index, at: option.filename.toLowerCase().indexOf(needle) }))
    .filter((entry) => entry.at >= 0)
    .sort((a, b) => Number(a.at !== 0) - Number(b.at !== 0) || a.index - b.index);
  return scored.slice(0, MAX_ATTACHMENT_MATCHES).map((entry) => entry.option);
}

/** Replaces the mention with `@filename ` and returns the new caret. */
export function applyMention(text: string, mention: Mention, caret: number, filename: string): { text: string; caret: number } {
  const inserted = `@${filename} `;
  const after = text.slice(caret).replace(/^ /, "");
  return { text: text.slice(0, mention.start) + inserted + after, caret: mention.start + inserted.length };
}
