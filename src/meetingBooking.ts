import type { MeetingProposal, Message } from "./domain";
import { safeWebUrl } from "./calendarTime";
import { foldQuotedHistoryHtml, foldQuotedHistoryText } from "./quotedHistory";
import { sanitizeMessageHtml } from "./SafeMessage";

// Recognize what the sender asks the recipient to do, independently of the
// booking provider. A generic URL or a conference join link is insufficient.
const BOOKING_LANGUAGE = /\b(?:(?:book|schedule|reserve|choose|pick|select)\s+(?:(?:a|an|the|your|our)\s+)?(?:time|slot|meeting|appointment|call|interview)|(?:booking|scheduling)\s+(?:page|link))\b/i;

/** Only a single, explicit booking destination from this suggestion's source email. */
export function meetingBookingUrl(proposal: MeetingProposal, message: Message | undefined): string | null {
  if (proposal.intent !== "schedule" || !message || message.id !== proposal.evidence.sourceMessageId) return null;
  if (!BOOKING_LANGUAGE.test(`${proposal.rawTimeLanguage}\n${proposal.evidence.excerpt}`)) return null;
  const urls = new Set<string>();
  // Reuse the reader's sanitized, quote-folded link analysis; never render the
  // sender's markup or reconstruct a destination from a provider name.
  const sanitized = sanitizeMessageHtml(message.bodyHtml);
  const shown = foldQuotedHistoryHtml(sanitized)?.visible ?? sanitized;
  const document = new DOMParser().parseFromString(shown, "text/html");
  for (const anchor of document.querySelectorAll("a[href]")) {
    if (anchor.closest("blockquote")) continue;
    const instruction = anchor.closest("p, li, td, div");
    const explicitLabel = BOOKING_LANGUAGE.test(anchor.textContent ?? "");
    const explicitInstruction = instruction?.querySelectorAll("a[href]").length === 1
      && BOOKING_LANGUAGE.test(instruction.textContent ?? "");
    if (!explicitLabel && !explicitInstruction) continue;
    const url = safeWebUrl(anchor.getAttribute("href"));
    if (url && !new URL(url).username && !new URL(url).password) urls.add(url);
  }
  const text = foldQuotedHistoryText(message.bodyText)?.visible ?? message.bodyText;
  for (const line of text.split(/\r?\n/)) {
    if (/^\s*>/.test(line) || !BOOKING_LANGUAGE.test(line)) continue;
    for (const match of line.matchAll(/https?:\/\/[^\s<>"\u201c\u201d]+/g)) {
      const url = safeWebUrl(match[0].replace(/[.,;:!?)\]]+$/, ""));
      if (url && !new URL(url).username && !new URL(url).password) urls.add(url);
    }
  }
  return urls.size === 1 ? [...urls][0] : null;
}
