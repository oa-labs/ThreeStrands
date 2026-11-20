import type { Account, Message, MessageAttachment } from "./domain";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import { formatCompactDate } from "./dateFormatting";
import { fileFitsNote, quoteWithinLimit } from "./addToNotePolicy";
import { extractBlockedImageUrls, sanitizeMessageHtml } from "./SafeMessage";

/** Where a message's passage came from, as the Adding preview words it. */
export function messageSource(message: Message, accounts: readonly Account[], now = new Date()): { label: string; dateLabel: string } {
  const sender = parseAddress(message.sender);
  const own = Boolean(message.sentByAccount) || accounts.some((account) => account.email.toLocaleLowerCase() === sender.email.toLocaleLowerCase());
  const dateLabel = formatCompactDate(new Date(message.sentAt), now);
  if (own) {
    const first = message.recipients.flatMap(splitAddressList)[0];
    const recipient = first ? parseAddress(first) : null;
    const name = recipient ? formatDisplayName(recipient.name || recipient.email) : "";
    return { label: name ? `your email to ${name}` : "your email", dateLabel };
  }
  return { label: formatDisplayName(sender.name || sender.email) || "an unknown sender", dateLabel };
}

/**
 * The files of a message a note can take: each attachment part and each
 * inline image that is a part of the message (cid references). Images the
 * message loads from the web are never copied, so they are only counted,
 * through the same sanitizer and blocked-source marker the reader uses (which
 * also parks `cid:` parts, already offered as attachments, and `data:` images).
 */
export function messageFiles(message: Message): { files: (MessageAttachment & { fits: boolean })[]; webImages: number } {
  const webImages = message.bodyHtml.trim() ? extractBlockedImageUrls(sanitizeMessageHtml(message.bodyHtml)).filter((url) => /^(https?:)?\/\//i.test(url)).length : 0;
  return { files: message.attachments.map((attachment) => ({ ...attachment, fits: fileFitsNote(attachment.size) })), webImages };
}

export const isImageAttachment = (attachment: MessageAttachment): boolean => attachment.mimeType.split(";", 1)[0]?.trim().toLocaleLowerCase().startsWith("image/") ?? false;

/** The text quoted when the whole message is included. */
export function wholeMessageText(message: Message): string {
  return message.bodyText.replace(/\r\n?/g, "\n").trim();
}

/** Why a passage can't be added, or null. */
export function quoteProblem(quote: string | null): string | null {
  return quote !== null && !quoteWithinLimit(quote) ? "This passage is too long to add. Select less of the message." : null;
}
