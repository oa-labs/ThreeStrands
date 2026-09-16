/**
 * Shared bare-URL/email detection used to turn plain text into real links,
 * both when composing (paste) and when displaying received messages whose
 * HTML never wrapped their URLs in <a> tags.
 */
export const LINKIFY_PATTERN = /(https?:\/\/[^\s<>"']+|www\.[^\s<>"']+|[\w.+-]+@[\w-]+\.[\w.-]+)/gi;

// Trailing punctuation (a sentence-ending period, a closing paren around the
// URL, ...) reads as part of the surrounding sentence, not the link.
export function trimTrailingPunctuation(value: string): { url: string; trailing: string } {
  const match = value.match(/[.,;:!?)\]}'"]+$/);
  if (!match) return { url: value, trailing: "" };
  return { url: value.slice(0, -match[0].length), trailing: match[0] };
}

export function linkHrefFor(url: string): string {
  const isEmail = url.includes("@") && !/^https?:\/\//i.test(url);
  return isEmail ? `mailto:${url}` : /^https?:\/\//i.test(url) ? url : `https://${url}`;
}
