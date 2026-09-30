import { htmlToPlainText } from "./htmlPlainText";

/**
 * Calendar providers often store event descriptions as HTML fragments
 * ("Dial in<br/>Phone: ..."). Event cards render descriptions as plain text,
 * so markup is flattened to readable text instead of showing literal tags.
 */

// Only descriptions that contain something tag-shaped are treated as HTML, so
// plain-text descriptions like "Budget < $5k" or "R&D sync" stay verbatim.
const HTML_MARKUP = /<\/?[a-z][a-z0-9-]*(?:\s[^<>]*)?\/?>|&(?:#\d+|#x[0-9a-f]+|[a-z]+);/i;

export function calendarDescriptionText(description: string): string {
  if (!HTML_MARKUP.test(description)) return description.trim();
  return htmlToPlainText(description, { whitespace: "collapse" });
}
