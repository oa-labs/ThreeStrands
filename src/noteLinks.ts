import { openMessageLink } from "./mailtoLink";

/**
 * The address a link in a note may open, or null. Note text arrives from
 * other devices and from pastes, so only web and email links leave the app;
 * `javascript:`, `file:`, and other schemes do nothing.
 */
export function noteLinkTarget(href: string): string | null {
  const value = href.trim();
  if (/^www\./i.test(value)) return noteLinkTarget(`https://${value}`);
  try {
    const url = new URL(value);
    return ["http:", "https:", "mailto:"].includes(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

/** Opens a note's link the way mail opens links: email addresses start a
 * draft here, web pages open in the browser. */
export function openNoteLink(href: string): void {
  const target = noteLinkTarget(href);
  if (target) openMessageLink(target);
}
