import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import { Image } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  backgroundImageUrl,
  blockedSrcAttr,
  safeColor,
  safeImageSrc,
  safeStyles,
} from "./emailSafeStyles";
import { sanitizeStyleSheet } from "./emailStyleSheet";
import { fontFamilyStack, type FontFamily } from "./settings";

type SafeMessageProps = {
  html: string;
  text?: string;
  loadImages?: boolean;
  /**
   * Fetches a remote image/background URL and resolves to a `data:` URI —
   * normally the native image proxy (`fetch_remote_image`), which validates
   * the URL and fetches it on the reader's behalf so the message iframe
   * never contacts the sender's host directly. Injected rather than
   * imported so this component stays decoupled from the data layer.
   */
  resolveImage?: (url: string) => Promise<string>;
  /** Namespaces embedded `cid:` image cache entries, which are only unique within a message. */
  imageCacheKey?: string;
  theme?: "light" | "dark";
  fontScale?: number;
  fontFamily?: FontFamily;
  tone?: "default" | "current" | "muted";
};

type QuotedHistoryBoundary =
  | { node: Element; kind: "element" }
  | { node: Text; kind: "text"; offset: number };

async function defaultResolveImage(): Promise<string> {
  throw new Error("Image loading is not configured for this SafeMessage instance");
}

// Styling for the isolated message document (see buildMessageDocument). This
// mirrors the palette and .message-body content rules in styles.css — kept
// here instead of styles.css because the iframe doesn't load the app's
// stylesheet, so this is the single source of truth for message content look.
//
// body's overflow-wrap is break-word, not anywhere: both allow breaking a
// long unbreakable token (a URL, a tracking id) as a last resort, but
// anywhere also shrinks a box's *minimum* content size for auto-layout
// purposes — inside a narrow fixed-width table cell (a numbered list's
// index column, say), that lets the layout treat even a short, ordinary
// 2-character string as breakable, splitting it across lines instead of
// letting the column render slightly wider than its width hint.
// break-word doesn't touch intrinsic sizing, so the column just widens
// instead, matching what other mail clients render.
const MESSAGE_DOCUMENT_STYLES = `
:root {
  color-scheme: dark;
  --text: #e8e8eb;
  --hover: #24242a;
  --code: #1b1b20;
  --border: #303037;
  --muted: #92929c;
  --secondary: #aaaab2;
  --body-text: #d2d2d7;
  --link: #aaa6ff;
  --quote-border: #66617f;
}
:root[data-theme="light"] {
  color-scheme: light;
  --text: #24242c;
  --hover: #e9e9f1;
  --code: #f1f1f6;
  --border: #d8d8e2;
  --muted: #626273;
  --secondary: #5e5e70;
  --body-text: #353541;
  --link: #5942b5;
  --quote-border: #aaa1c7;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  color: var(--body-text);
  font-size: calc(15px * var(--font-scale, 1));
  line-height: 1.7;
  overflow-wrap: break-word;
  overflow-x: auto;
}
body[data-tone="current"] { color: var(--text); }
body[data-tone="muted"] { color: var(--muted); }
[hidden] { display: none; }
img { max-width: 100%; height: auto; }
img:not([src]) { display: inline-block; min-width: 24px; min-height: 24px; border: 1px dashed var(--border); background: var(--hover); vertical-align: middle; }
body > :first-child { margin-top: 0; }
body > :last-child { margin-bottom: 0; }
p { margin: 0 0 1em; }
h1, h2, h3, h4, h5, h6 { margin: 1.3em 0 .5em; line-height: 1.3; color: var(--text); }
h1 { font-size: 1.6em; }
h2 { font-size: 1.35em; }
h3 { font-size: 1.15em; }
ul, ol { padding-inline-start: 1.6em; }
li + li { margin-top: .25em; }
table { max-width: 100%; border-collapse: collapse; font-size: inherit; }
td, th { padding: 6px 10px; vertical-align: top; }
th { text-align: start; }
caption { text-align: start; font-weight: 600; margin-bottom: .5em; }
pre { max-width: 100%; overflow-x: auto; padding: 14px 16px; border: 1px solid var(--border); border-radius: 8px; background: var(--code); line-height: 1.5; }
code { font: .9em ui-monospace, SFMono-Regular, Menlo, monospace; }
:not(pre) > code { padding: 2px 4px; border-radius: 4px; background: var(--code); }
hr { margin: 1.5em 0; border: 0; border-top: 1px solid var(--border); }
a { color: var(--link); text-underline-offset: 3px; }
a:focus-visible { outline: 2px solid var(--link); outline-offset: 3px; }
blockquote { margin-left: 0; padding-left: 16px; border-left: 2px solid var(--quote-border); color: var(--secondary); }
`;

const MESSAGE_DOCUMENT_CSP = [
  "default-src 'none'",
  "script-src 'none'",
  "style-src 'unsafe-inline'",
  // Only data: — every remote image/background is resolved to one through
  // the native proxy before it ever reaches this document, so the iframe
  // itself never needs to make a network request for an image.
  "img-src data:",
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
].join("; ");

function buildMessageDocument(bodyHtml: string, options: {
  theme: "light" | "dark";
  fontScale: number;
  fontFamily: FontFamily;
  tone: "default" | "current" | "muted";
  emailStyleSheet: string;
}): string {
  const { theme, fontScale, fontFamily, tone, emailStyleSheet } = options;
  const fontStyle = fontFamilyStack(fontFamily)
    .replaceAll("&", "&amp;")
    .replaceAll('"', "&quot;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
  return `<!doctype html>
<html data-theme="${theme}">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${MESSAGE_DOCUMENT_CSP}">
<style>${MESSAGE_DOCUMENT_STYLES}</style>
${emailStyleSheet ? `<style>${emailStyleSheet}</style>\n` : ""}</head>
<body data-tone="${tone}" style="font-family: ${fontStyle}; --font-scale: ${fontScale};">
${bodyHtml}
</body>
</html>`;
}

/**
 * Pulls every <style> block out of the sender's original HTML (before
 * DOMPurify strips them, same as it always has) and reduces each to the
 * allowlisted subset sanitizeStyleSheet permits. Parsing with DOMParser
 * here is inert — it never executes scripts or fetches resources, it's
 * just a tree we read `<style>` text back out of.
 */
export function extractSafeStyleSheet(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  return Array.from(doc.querySelectorAll("style"))
    .map((style) => sanitizeStyleSheet(style.textContent ?? ""))
    .filter(Boolean)
    .join("\n");
}

const allowedTags = [
  "a", "b", "blockquote", "br", "caption", "code", "col", "colgroup",
  "dd", "del", "div", "dl", "dt", "em", "h1", "h2", "h3", "h4",
  "h5", "h6", "hr", "i", "img", "kbd", "li", "ol", "p", "pre", "s",
  "small", "span", "strong", "sub", "sup", "table", "tbody", "td",
  "tfoot", "th", "thead", "tr", "u", "ul",
];

// Block-level elements that marketing templates commonly use as empty
// "spacer" wrappers (padding/margin only, no text or meaningful children).
// Table cells are handled separately since removing them would break
// column alignment for the rest of the row.
const spacerTags = new Set([
  "div", "p", "span", "li", "blockquote", "h1", "h2", "h3", "h4", "h5", "h6", "dd", "dt",
]);

const dimensionAttributeTags = new Set(["img", "table", "td", "th"]);

function safeDimension(value: string, allowPercent: boolean): string | null {
  const match = value.trim().match(/^(\d+(?:\.\d+)?)(%)?$/);
  if (!match) return null;

  const amount = Number(match[1]);
  const isPercent = match[2] === "%";
  if (!Number.isFinite(amount) || amount < 0) return null;
  if (isPercent) return allowPercent && amount <= 100 ? `${amount}%` : null;
  return amount <= 4096 ? `${amount}` : null;
}

// Same bounds as safeDimension, but for a CSS length (which carries its own
// unit) rather than a bare HTML width/height attribute.
function safeCssLength(value: string, allowPercent: boolean): string | null {
  const safe = safeDimension(value.trim().replace(/px$/, ""), allowPercent);
  if (safe === null) return null;
  return safe.endsWith("%") ? safe : `${safe}px`;
}

export function decodeHtmlEntities(text: string): string {
  const container = document.createElement("textarea");
  container.innerHTML = text;
  return container.value;
}

const LINKIFY_PATTERN = /(https?:\/\/[^\s<>"']+|www\.[^\s<>"']+|[\w.+-]+@[\w-]+\.[\w.-]+)/gi;

// Trailing punctuation (a sentence-ending period, a closing paren around the
// URL, ...) reads as part of the surrounding sentence, not the link.
function trimTrailingPunctuation(value: string): { url: string; trailing: string } {
  const match = value.match(/[.,;:!?)\]}'"]+$/);
  if (!match) return { url: value, trailing: "" };
  return { url: value.slice(0, -match[0].length), trailing: match[0] };
}

/**
 * Turns bare URLs, www.-domains, and email addresses in a plain-text message
 * body into clickable links, routed through the same `openUrl` (OS browser /
 * default mail client) path HTML message bodies use.
 */
export function linkifyText(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let lastIndex = 0;
  for (const match of text.matchAll(LINKIFY_PATTERN)) {
    const index = match.index ?? 0;
    const { url, trailing } = trimTrailingPunctuation(match[0]);
    if (!url) continue;
    if (index > lastIndex) nodes.push(text.slice(lastIndex, index));
    const isEmail = url.includes("@") && !/^https?:\/\//i.test(url);
    const href = isEmail ? `mailto:${url}` : /^https?:\/\//i.test(url) ? url : `https://${url}`;
    nodes.push(
      <a
        key={`link-${index}`}
        href={href}
        onClick={(event) => {
          event.preventDefault();
          void openUrl(href);
        }}
      >
        {url}
      </a>,
    );
    lastIndex = index + match[0].length;
    if (trailing) nodes.push(trailing);
  }
  if (nodes.length === 0) return [text];
  if (lastIndex < text.length) nodes.push(text.slice(lastIndex));
  return nodes;
}

function isBlank(element: Element): boolean {
  return (element.textContent ?? "").replace(/\s+/g, "") === "";
}

// A lone non-breaking space (or run of them) inside an otherwise-empty
// element is a deliberate line-height spacer — templates commonly use
// "<p>&nbsp;</p>" to reserve a blank line's height between sections since
// margins are often reset to 0. isBlank() above treats it as whitespace (JS's
// \s matches U+00A0), which is right for zeroing an empty table cell's
// padding, but wrong for deciding whether to remove the element entirely:
// that would delete the sender's spacing outright instead of just rendering
// it, unlike every other mail client.
function isPureSpacingChar(element: Element): boolean {
  return /^\u00A0+$/.test(element.textContent ?? "");
}

// A spacer div/span is invisible by definition \u2014 it exists only to reserve
// blank space. An empty, childless element that paints a background is a
// real decorative mark instead (a colored dot, a swatch, a divider bar),
// even though it's shaped exactly like a spacer to the checks above.
function hasVisibleFill(element: HTMLElement): boolean {
  const backgroundColor = element.style.getPropertyValue("background-color").trim().toLowerCase();
  if (backgroundColor && backgroundColor !== "transparent") return true;
  return element.hasAttribute(blockedSrcAttr);
}

// Empty fixed-height blocks are a standard HTML-email spacing primitive.
// Keep modest pixel spacers while still collapsing oversized/percentage
// blocks that would create unbounded dead whitespace. The style sanitizer
// has already normalized the declaration by the time this runs.
function hasIntentionalSpacerHeight(element: HTMLElement): boolean {
  const match = element.style.getPropertyValue("height").trim().match(/^(\d+(?:\.\d+)?)px$/);
  return match !== null && Number(match[1]) > 0 && Number(match[1]) <= 32;
}

export function sanitizeMessageHtml(html: string): string {
  const fragment = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: allowedTags,
    // class/id are only useful now that <style> blocks are (narrowly)
    // supported below — otherwise there's no stylesheet for them to
    // address. They're plain strings with no special handling anywhere
    // else in this file: DOMPurify already keeps them from carrying
    // markup, and the iframe never loads the app's own stylesheet, so a
    // sender's class name can't collide with anything of ours.
    ALLOWED_ATTR: ["align", "alt", "bgcolor", "cellpadding", "cellspacing", "class", "colspan", "dir", "height", "hidden", "href", "id", "rowspan", "src", "start", "style", "title", "valign", "width"],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    FORBID_TAGS: ["form", "script", "style", "svg"],
    RETURN_DOM_FRAGMENT: true,
  });

  fragment.querySelectorAll<HTMLElement>("*").forEach((element) => {
    const original = element.style;
    const declarations: string[] = [];
    // Email preview text is commonly kept in the body for inbox snippets and
    // hidden inline. Preserve that intent without retaining arbitrary CSS.
    if (original.display.trim().toLowerCase() === "none"
      || ["hidden", "collapse"].includes(original.visibility.trim().toLowerCase())) {
      element.setAttribute("hidden", "");
    }
    for (const [property, pattern] of Object.entries(safeStyles)) {
      const value = original.getPropertyValue(property).trim().toLowerCase();
      if (pattern.test(value)) declarations.push(`${property}: ${value}`);
    }
    // Same URL scheme check as <img src>. Every remote background, like
    // every remote <img>, is always parked behind the blocked-src marker
    // here — resolving it to a real value is the caller's job (see
    // applyResolvedImages), once the native proxy has fetched it.
    const backgroundImageMatch = original.getPropertyValue("background-image").trim().match(backgroundImageUrl);
    if (backgroundImageMatch) {
      const backgroundUrl = backgroundImageMatch[1] ?? backgroundImageMatch[2] ?? backgroundImageMatch[3] ?? "";
      if (safeImageSrc.test(backgroundUrl) && !/["']/.test(backgroundUrl)) {
        element.setAttribute(blockedSrcAttr, backgroundUrl);
      }
    }
    const tag = element.tagName.toLowerCase();
    if (!dimensionAttributeTags.has(tag)) element.removeAttribute("width");
    if (tag !== "img") element.removeAttribute("height");

    const width = element.getAttribute("width");
    if (width !== null) {
      const safeWidth = safeDimension(width, true);
      if (safeWidth === null) element.removeAttribute("width");
      else element.setAttribute("width", safeWidth);
    }
    const height = element.getAttribute("height");
    if (height !== null) {
      const safeHeight = safeDimension(height, false);
      if (safeHeight === null) element.removeAttribute("height");
      else element.setAttribute("height", safeHeight);
    }
    // Some templates size elements via CSS instead of the width/height
    // attributes above; apply the same bounds either way.
    if (dimensionAttributeTags.has(tag)) {
      const styleWidth = original.getPropertyValue("width").trim();
      const safeWidth = styleWidth ? safeCssLength(styleWidth, true) : null;
      if (safeWidth) declarations.push(`width: ${safeWidth}`);
    }
    if (tag === "img") {
      const styleHeight = original.getPropertyValue("height").trim();
      const safeHeight = styleHeight ? safeCssLength(styleHeight, false) : null;
      if (safeHeight) declarations.push(`height: ${safeHeight}`);
    }
    if (tag === "table") {
      for (const attr of ["cellpadding", "cellspacing"]) {
        const value = element.getAttribute(attr);
        if (value === null) continue;
        if (!/^\d+$/.test(value.trim())) element.removeAttribute(attr);
        else element.setAttribute(attr, String(Math.min(Number(value), 32)));
      }
      // `cellpadding` is a layout table's explicit declaration of per-cell
      // spacing (almost always 0, for a hairline-tight row of icons/text) —
      // but a browser only honors it as a low-priority presentational hint,
      // which this document's own `td, th { padding: 6px 10px }` base rule
      // (see MESSAGE_DOCUMENT_STYLES) always outranks regardless of source
      // order. Mirror it as a real inline style on each direct cell, one
      // side at a time so a cell's own explicit padding on any side is left
      // alone, so it wins the cascade the way the sender's intended
      // rendering would have.
      const cellPadding = element.getAttribute("cellpadding");
      if (cellPadding !== null) {
        const rows = element.querySelectorAll(":scope > tr, :scope > tbody > tr, :scope > thead > tr, :scope > tfoot > tr");
        for (const row of rows) {
          for (const cell of Array.from(row.children) as HTMLElement[]) {
            const cellTag = cell.tagName.toLowerCase();
            if (cellTag !== "td" && cellTag !== "th") continue;
            for (const side of ["padding-top", "padding-right", "padding-bottom", "padding-left"]) {
              if (!cell.style.getPropertyValue(side)) cell.style.setProperty(side, `${cellPadding}px`);
            }
          }
        }
      }
    }
    const bgcolor = element.getAttribute("bgcolor");
    if (bgcolor !== null) {
      const value = bgcolor.trim().toLowerCase();
      if (safeColor.test(value)) element.setAttribute("bgcolor", value);
      else element.removeAttribute("bgcolor");
    }
    // Empty spacer cells carry no content, so zero their spacing outright
    // instead of just capping it, rather than let it render as a dead gap.
    const isEmptyCell = (tag === "td" || tag === "th") && element.children.length === 0 && isBlank(element);
    // Senders often zero out a browser default (e.g. a <p>'s ~1em margin)
    // using the same unit as their own font-size — "margin-bottom: 0em" is
    // as common as "0px". Restricting this to px only silently dropped
    // that reset, letting the UA default margin resurface instead of the
    // zero the sender asked for. The cap is unit-aware since 1em and 1px
    // aren't the same amount of space; 0 is always safe regardless of unit.
    const spacingUnitCaps: Record<string, number> = { px: 32, em: 2, rem: 2, "%": 50 };
    for (const property of ["padding-top", "padding-right", "padding-bottom", "padding-left", "margin-top", "margin-bottom"]) {
      const value = original.getPropertyValue(property).trim();
      const match = value.match(/^(\d+(?:\.\d+)?)(px|em|rem|%)$/);
      if (match) {
        const amount = isEmptyCell ? 0 : Math.min(Number(match[1]), spacingUnitCaps[match[2]]);
        declarations.push(`${property}: ${amount}${match[2]}`);
      }
    }
    element.removeAttribute("style");
    if (declarations.length) element.setAttribute("style", declarations.join("; "));

    if (tag === "a") {
      const href = element.getAttribute("href") ?? "";
      if (/^(https?:\/\/|mailto:|tel:)/i.test(href)) {
        // Every click is already intercepted in the frame's own onClick
        // handler (see handleLoad below), which calls preventDefault and
        // routes the href through the native opener instead of letting the
        // browser navigate — a target="_blank" is never meant to be
        // followed natively. DOMPurify already strips any target the
        // sender sent (it's not in ALLOWED_ATTR above); this used to add
        // one back, but WKWebView treats target="_blank" on an anchor
        // inside this sandboxed iframe (no allow-popups) as a popup
        // request and can swallow the click before our handler's
        // preventDefault runs, leaving the link inert. Not setting it
        // avoids that native short-circuit entirely.
        element.setAttribute("rel", "noopener noreferrer");
      } else {
        element.removeAttribute("href");
      }
    }

    if (tag === "img") {
      const src = element.getAttribute("src") ?? "";
      element.removeAttribute("src");
      if (!safeImageSrc.test(src)) element.remove();
      else element.setAttribute(blockedSrcAttr, src);
    }
  });

  // Now that invalid images are gone and styles are resolved, remove any
  // block-level spacer elements left with no text and no remaining children.
  // Processed in reverse document order so children are handled before their
  // ancestors, letting nested spacer stacks collapse in one pass.
  for (const element of Array.from(fragment.querySelectorAll<HTMLElement>("*")).reverse()) {
    if (!fragment.contains(element)) continue;
    if (
      spacerTags.has(element.tagName.toLowerCase())
      && element.children.length === 0
      && isBlank(element)
      && !isPureSpacingChar(element)
      && !hasVisibleFill(element)
      && !hasIntentionalSpacerHeight(element)
    ) {
      element.remove();
    }
  }

  const container = document.createElement("div");
  container.append(fragment);
  return container.innerHTML;
}

const quotedHistorySelector = [
  ".gmail_quote",
  ".protonmail_quote",
  ".yahoo_quoted",
  '[id^="yahoo_quoted"]',
  "#divRplyFwdMsg",
  ".OutlookMessageHeader",
  ".moz-cite-prefix",
  "blockquote",
].join(", ");

const quotedHistoryMarker = /(?:^|\n)\s*(?:(?:[-—_]{2,})\s*)?(?:original message|forwarded message|begin forwarded message)(?:\s*(?:[-—_]{2,}))?\s*(?:\n|$)/i;
const wroteMarker = /(?:^|\n)\s*On\s+[^\n]{1,500}\s+wrote:\s*(?:\n|$)/i;

function nodeComesBefore(left: Node, right: Node): boolean {
  return Boolean(left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING);
}

/**
 * Returns the message HTML before a mail client's quoted-reply section.
 * The full sanitized HTML remains available to reveal after the reader asks
 * for it; only this shorter copy is placed in the iframe initially.
 */
export function collapseQuotedHistoryHtml(html: string): string | null {
  const container = document.createElement("div");
  container.innerHTML = html;
  const candidates: QuotedHistoryBoundary[] = Array.from(
    container.querySelectorAll(quotedHistorySelector),
    (node) => ({ node, kind: "element" as const }),
  );

  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let textNode = walker.nextNode();
  while (textNode) {
    const text = textNode.textContent ?? "";
    const marker = quotedHistoryMarker.exec(text) ?? wroteMarker.exec(text);
    if (marker?.index !== undefined) {
      candidates.push({ node: textNode as Text, kind: "text", offset: marker.index });
    }
    textNode = walker.nextNode();
  }

  if (candidates.length === 0) return null;
  candidates.sort((left, right) => {
    if (left.node === right.node) {
      const leftOffset = left.kind === "text" ? left.offset : 0;
      const rightOffset = right.kind === "text" ? right.offset : 0;
      return leftOffset - rightOffset;
    }
    return nodeComesBefore(left.node, right.node) ? -1 : 1;
  });

  for (const boundary of candidates) {
    const range = document.createRange();
    range.setStart(container, 0);
    if (boundary.kind === "text") range.setEnd(boundary.node, boundary.offset);
    else range.setEndBefore(boundary.node);

    const visibleContainer = document.createElement("div");
    visibleContainer.append(range.cloneContents());
    const hasVisibleContent = Boolean(visibleContainer.textContent?.trim())
      || visibleContainer.querySelector("img") !== null;
    if (hasVisibleContent) return visibleContainer.innerHTML;
  }
  return null;
}

/** Returns the part of a plain-text reply before its quoted history. */
export function collapseQuotedHistoryText(text: string): string | null {
  const marker = quotedHistoryMarker.exec(text) ?? wroteMarker.exec(text);
  if (!marker?.index) return null;
  const visible = text.slice(0, marker.index).trimEnd();
  return visible.trim() ? visible : null;
}

/** Every URL currently parked behind a blocked-src marker, deduplicated. */
export function extractBlockedImageUrls(html: string): string[] {
  const container = document.createElement("div");
  container.innerHTML = html;
  const urls = new Set<string>();
  container.querySelectorAll(`[${blockedSrcAttr}]`).forEach((element) => {
    const url = element.getAttribute(blockedSrcAttr);
    if (url) urls.add(url);
  });
  return Array.from(urls);
}

/**
 * Fills in whichever blocked-src markers have a resolved `data:` URI
 * available, leaving any without one (not yet fetched, or the fetch failed)
 * blocked exactly as sanitizeMessageHtml left them.
 */
export function applyResolvedImages(html: string, resolved: ReadonlyMap<string, string>): string {
  if (resolved.size === 0) return html;
  const container = document.createElement("div");
  container.innerHTML = html;
  container.querySelectorAll<HTMLElement>(`[${blockedSrcAttr}]`).forEach((element) => {
    const url = element.getAttribute(blockedSrcAttr);
    const dataUri = url ? resolved.get(url) : undefined;
    if (!dataUri) return;
    element.removeAttribute(blockedSrcAttr);
    if (element.tagName.toLowerCase() === "img") {
      element.setAttribute("src", dataUri);
    } else {
      element.style.setProperty("background-image", `url("${dataUri}")`);
    }
  });
  return container.innerHTML;
}

// Shared across every SafeMessage instance for the life of the session, so
// an asset referenced repeatedly (a sender's logo reused across many
// emails, a tracking pixel repeated within one) is fetched at most once —
// on top of the native proxy's own cache, this also dedupes concurrent
// requests for the same URL across separate messages.
const resolvedImagePromises = new Map<string, Promise<string>>();

export function SafeMessage({
  html,
  text = "",
  loadImages = false,
  resolveImage = defaultResolveImage,
  imageCacheKey = "",
  theme = "dark",
  fontScale = 1,
  fontFamily = "system",
  tone = "default",
}: SafeMessageProps) {
  const [imagesAllowedForMessage, setImagesAllowedForMessage] = useState(false);
  const [quotedHistoryExpanded, setQuotedHistoryExpanded] = useState(false);
  const imagesAllowed = loadImages || imagesAllowedForMessage;
  const sanitized = useMemo(() => sanitizeMessageHtml(html), [html]);
  const collapsedHtml = useMemo(() => collapseQuotedHistoryHtml(sanitized), [sanitized]);
  const collapsedText = useMemo(() => collapseQuotedHistoryText(text), [text]);
  const hasCollapsedHistory = collapsedHtml !== null || (!sanitized.trim() && collapsedText !== null);
  const renderedHtml = !quotedHistoryExpanded && collapsedHtml !== null ? collapsedHtml : sanitized;
  const blockedUrls = useMemo(() => extractBlockedImageUrls(renderedHtml), [renderedHtml]);
  const resolvableUrls = useMemo(
    () => imagesAllowed ? blockedUrls : blockedUrls.filter((url) => /^cid:/i.test(url)),
    [imagesAllowed, blockedUrls],
  );
  const hasContent = useMemo(() => {
    const container = document.createElement("div");
    container.innerHTML = renderedHtml;
    return Boolean(container.textContent?.trim()) || container.querySelector("img") !== null;
  }, [renderedHtml]);
  const hasBlockedImages = !imagesAllowed && blockedUrls.some((url) => !/^cid:/i.test(url));

  const [resolvedImages, setResolvedImages] = useState<Map<string, string>>(new Map());

  useEffect(() => {
    if (resolvableUrls.length === 0) return;
    let cancelled = false;

    void Promise.all(resolvableUrls.map(async (url) => {
      const cacheKey = /^cid:/i.test(url) ? `${imageCacheKey}\0${url}` : url;
      let pending = resolvedImagePromises.get(cacheKey);
      if (!pending) {
        pending = resolveImage(url);
        resolvedImagePromises.set(cacheKey, pending);
        // Don't let a transient failure permanently poison the shared
        // cache — a later retry (e.g. a fresh "Load images" click) should
        // get a real second attempt, not the same rejected promise.
        pending.catch(() => resolvedImagePromises.delete(cacheKey));
      }
      try {
        return [url, await pending] as const;
      } catch {
        return null;
      }
    })).then((results) => {
      if (cancelled) return;
      const resolved = results.filter((entry): entry is readonly [string, string] => entry !== null);
      if (resolved.length === 0) return;
      setResolvedImages((previous) => {
        if (resolved.every(([url, dataUri]) => previous.get(url) === dataUri)) return previous;
        const next = new Map(previous);
        for (const [url, dataUri] of resolved) next.set(url, dataUri);
        return next;
      });
    });

    return () => {
      cancelled = true;
    };
  }, [imageCacheKey, resolvableUrls, resolveImage]);

  const displayHtml = useMemo(
    () => applyResolvedImages(renderedHtml, resolvedImages),
    [renderedHtml, resolvedImages],
  );
  const emailStyleSheet = useMemo(() => extractSafeStyleSheet(html), [html]);

  const doc = useMemo(
    () => buildMessageDocument(displayHtml, { theme, fontScale, fontFamily, tone, emailStyleSheet }),
    [displayHtml, theme, fontScale, fontFamily, tone, emailStyleSheet],
  );

  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const [frameHeight, setFrameHeight] = useState(0);

  const handleLoad = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;

    const frame = frameRef.current;
    const frameDoc = frame?.contentDocument;
    if (!frameDoc) return;

    const resize = () => {
      const height = frameDoc.documentElement?.scrollHeight ?? frameDoc.body?.scrollHeight ?? 0;
      setFrameHeight(height);
    };
    resize();

    let observer: ResizeObserver | undefined;
    if (frameDoc.body && typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(resize);
      observer.observe(frameDoc.body);
    }

    const onClick = (event: MouseEvent) => {
      const target = event.target as HTMLElement | null;
      const href = target?.closest("a")?.getAttribute("href");
      if (!href) return;
      event.preventDefault();
      void openUrl(href);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      // Keyboard events do not cross iframe boundaries. Forward input from
      // this read-only document so every global application shortcut keeps
      // working after the reader clicks or tabs into an email body.
      const forwarded = new KeyboardEvent("keydown", {
        key: event.key,
        code: event.code,
        location: event.location,
        ctrlKey: event.ctrlKey,
        shiftKey: event.shiftKey,
        altKey: event.altKey,
        metaKey: event.metaKey,
        repeat: event.repeat,
        isComposing: event.isComposing,
        bubbles: true,
        cancelable: true,
      });
      if (!window.dispatchEvent(forwarded)) event.preventDefault();
    };
    frameDoc.addEventListener("click", onClick);
    frameDoc.addEventListener("keydown", onKeyDown);

    cleanupRef.current = () => {
      observer?.disconnect();
      frameDoc.removeEventListener("click", onClick);
      frameDoc.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  useEffect(() => () => cleanupRef.current?.(), []);

  useEffect(() => {
    setQuotedHistoryExpanded(false);
  }, [html, text]);

  const quotedHistoryButton = hasCollapsedHistory && !quotedHistoryExpanded ? (
    <button
      type="button"
      className="quoted-history-toggle"
      aria-label="Show quoted content"
      title="Show quoted content"
      onClick={() => setQuotedHistoryExpanded(true)}
    >
      &hellip;
    </button>
  ) : null;

  if (!hasContent) {
    const decoded = decodeHtmlEntities(text);
    const visibleText = !quotedHistoryExpanded && collapsedText !== null ? collapsedText : decoded;
    return (
      <>
        <div className="message-body message-body-plain" data-testid="message-body">
          {visibleText ? linkifyText(visibleText) : "No message content."}
        </div>
        {quotedHistoryButton}
      </>
    );
  }

  return (
    <>
      {hasBlockedImages ? (
        <div className="message-images-notice">
          <span>Images are blocked in this message.</span>
          <button type="button" onClick={() => setImagesAllowedForMessage(true)}>
            <Image size={14} /> Load images
          </button>
        </div>
      ) : null}
      <iframe
        ref={frameRef}
        data-testid="message-body"
        className="message-body"
        title="Message content"
        // allow-scripts sounds dangerous for untrusted email HTML, but the
        // document's own CSP above (script-src 'none') independently blocks
        // every script in it from running — DOMPurify has also already
        // stripped <script>, event-handler attributes, and javascript:
        // URLs. What this flag actually enables is the click/keydown
        // listeners handleLoad attaches from the parent below: WebKit
        // refuses to invoke ANY listener bound to a document whose sandbox
        // omits allow-scripts, including ones added by the parent, which
        // silently broke every shortcut once the reader clicked or
        // selected text in the message body (see #handleLoad).
        sandbox="allow-same-origin allow-scripts"
        referrerPolicy="no-referrer"
        srcDoc={doc}
        onLoad={handleLoad}
        style={{ height: frameHeight }}
      />
      {quotedHistoryButton}
    </>
  );
}
