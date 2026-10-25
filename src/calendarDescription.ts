import { safeWebUrl } from "./calendarTime";
import { htmlToPlainText } from "./htmlPlainText";

/**
 * Calendar providers often store event descriptions as HTML fragments
 * ("Dial in<br/>Phone: ..."). Event cards render descriptions as plain text,
 * so markup is flattened to readable text instead of showing literal tags.
 */

// Only descriptions that contain something tag-shaped are treated as HTML, so
// plain-text descriptions like "Budget < $5k" or "R&D sync" stay verbatim.
const HTML_MARKUP = /<\/?[a-z][a-z0-9-]*(?:\s[^<>]*)?\/?>|&(?:#\d+|#x[0-9a-f]+|[a-z]+);/i;

// Decorative colon/tilde rules are presentation separators, regardless of the
// calendar provider. Match whole lines only so inline punctuation stays intact.
const SEPARATOR_LINE = /^[ \t]*[-:~]*:~:~[-:~]*[ \t]*(?:\r?\n|$)/gm;

export function calendarDescriptionText(description: string): string {
  const text = HTML_MARKUP.test(description)
    ? htmlToPlainText(description, { whitespace: "collapse" })
    : description;
  return text.replace(SEPARATOR_LINE, "").trim();
}

/** A whole, explicitly labelled join line supplies a meeting link without guessing from a host. */
export function calendarInvitationDetails(location: string | null, description: string | null): { location: string; description: string; joinUrl: string | null } {
  const text = description ? calendarDescriptionText(description) : "";
  const lines = text.split("\n");
  const joinLine = lines.findIndex((line) => /^(?:Join(?: with [^:\n]+)?|Meeting link|Video call):\s*https?:\/\/\S+\s*$/i.test(line.trim()));
  const written = joinLine < 0 ? null : lines[joinLine].trim().replace(/^[^:]+:\s*/, "");
  const joinUrl = safeWebUrl(written);
  if (!joinUrl) return { location: location ?? "", description: text, joinUrl: null };
  // Only remove an identical URL token. A different path, query, or trailing text stays intact.
  const place = (location ?? "").replace(/https?:\/\/[^\s<>";]+/g, (url) => url === written ? "" : url)
    .replace(/^[\s,;·]+|[\s,;·]+$/g, "");
  return { location: place, description: lines.filter((_, index) => index !== joinLine).join("\n").trim(), joinUrl };
}
