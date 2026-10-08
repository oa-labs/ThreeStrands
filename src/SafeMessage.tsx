import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import { Image } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  backgroundImageUrl,
  blockedSrcAttr,
  safeColor,
  safeImageSrc,
  safeStyleProperties,
  emailFrameHeight,
  EMAIL_IMAGE_LIMITS,
  sanitizeCssDeclaration,
  sanitizeHtmlDimension,
  parseEmailMinimumFontSize,
} from "./emailRenderingPolicy";
import { createEmailFontSizeController } from "./emailMinimumFontSize";
import { sanitizeStyleSheet } from "./emailStyleSheet";
import {
  collapseQuotedHistoryHtml,
  collapseQuotedHistoryText,
  foldQuotedHistoryHtml,
  foldQuotedHistoryText,
  QUOTED_HISTORY_FOLD_ATTRIBUTE,
  type PriorThreadText,
} from "./quotedHistory";
import { LINKIFY_PATTERN, linkHrefFor, trimTrailingPunctuation } from "./linkify";
import { fontFamilyStack, type FontFamily } from "./settings";
import { ICON_SIZE } from "./iconSizes";

export { collapseQuotedHistoryHtml, collapseQuotedHistoryText };

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
  emailMinimumFontSize?: number;
  fontFamily?: FontFamily;
  tone?: "default" | "current" | "muted";
  /** Text from earlier messages in the conversation; lets repeated text (such as a signature) fold. */
  priorThreadText?: PriorThreadText;
  /** Called with an image's resolved `src` when the reader clicks it in the message body. */
  onImageClick?: (src: string) => void;
  /**
   * Called when the reader presses a plain, unmodified Enter inside the
   * message body. Keydown events don't cross iframe boundaries, and the
   * generic keydown forwarding below only reaches window-level shortcut
   * listeners, not a parent component's local Enter handler (e.g. the
   * expand/collapse toggle button) — so without this, Enter silently stops
   * collapsing the message once the reader has clicked into its body.
   */
  onEnterKey?: () => void;
};

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
/** Room reserved at the fold for the quoted-history toggle: its height plus vertical margins in styles.css. */
const QUOTED_HISTORY_FOLD_HEIGHT = 28;

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
:where([hidden]) { display: none; }
:where(img) { max-width: 100%; }
:where(img[src]) { cursor: zoom-in; }
:where(img:not([src])) { display: inline-block; min-width: 24px; min-height: 24px; border: 1px dashed var(--border); background: var(--hover); vertical-align: middle; }
/* Cosmetic text-decoration geometry only; it cannot fetch resources, execute
   code, escape the iframe, or create an interactive surface. */
:where(a) { color: var(--link); text-underline-offset: 3px; text-decoration-skip-ink: none; }
:where(a:focus-visible) { outline: 2px solid var(--link); outline-offset: 3px; }
:where(pre) { max-width: 100%; overflow-x: auto; }
:where(code) { font: .9em ui-monospace, SFMono-Regular, Menlo, monospace; }
:where(hr) { border: 0; border-top: 1px solid var(--border); }
:where(table) { max-width: 100%; }
:where(.email-root) { max-width: 100%; overflow-wrap: break-word; }
/* Reserves room for the reader's quoted-history toggle, which the parent
   overlays here. The marker is inserted by ThreeStrands after sanitization
   (senders cannot emit data attributes), is empty, and only takes up space. */
span[${QUOTED_HISTORY_FOLD_ATTRIBUTE}] {
  display: block !important;
  height: ${QUOTED_HISTORY_FOLD_HEIGHT}px !important;
  margin: 0 !important;
  padding: 0 !important;
  border: 0 !important;
}
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
<div class="email-root" data-email-root data-theme="${theme}">
${bodyHtml}
</div>
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
export function extractSafeStyleSheet(html: string, theme?: "light" | "dark"): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  return Array.from(doc.querySelectorAll("style"))
    .map((style) => sanitizeStyleSheet(style.textContent ?? "", theme))
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

const dimensionAttributeTags = new Set(["img", "table", "td", "th"]);

export function decodeHtmlEntities(text: string): string {
  const container = document.createElement("textarea");
  container.innerHTML = text;
  return container.value;
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
    const href = linkHrefFor(url);
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

/**
 * Senders' HTML bodies routinely contain bare URLs that were never wrapped
 * in an <a> — many mail clients don't autolink either. Walk the sanitized
 * fragment's text nodes (skipping anything already inside a link) and wrap
 * matches in real anchors, same rule set as the plain-text fallback above.
 */
function linkifyTextNodes(root: Node): void {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => node.parentElement?.closest("a") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const targets: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    // search(), not test(): test() on this shared global pattern leaves
    // lastIndex past the match, and matchAll() — here and in every other
    // caller — starts from that copied lastIndex and skips the link.
    if ((node.textContent ?? "").search(LINKIFY_PATTERN) !== -1) targets.push(node as Text);
  }
  targets.forEach((textNode) => {
    const text = textNode.textContent ?? "";
    const replacement = document.createDocumentFragment();
    let lastIndex = 0;
    for (const match of text.matchAll(LINKIFY_PATTERN)) {
      const index = match.index ?? 0;
      const { url, trailing } = trimTrailingPunctuation(match[0]);
      if (!url) continue;
      if (index > lastIndex) replacement.append(document.createTextNode(text.slice(lastIndex, index)));
      const anchor = document.createElement("a");
      anchor.setAttribute("href", linkHrefFor(url));
      anchor.setAttribute("rel", "noopener noreferrer");
      anchor.textContent = url;
      replacement.append(anchor);
      lastIndex = index + match[0].length;
      if (trailing) replacement.append(document.createTextNode(trailing));
    }
    if (lastIndex < text.length) replacement.append(document.createTextNode(text.slice(lastIndex)));
    textNode.replaceWith(replacement);
  });
}

/**
 * The href of the link containing `target`, if any. Duck-typed rather than
 * `instanceof Element` because targets inside the message iframe belong to
 * that frame's realm, not the parent's.
 */
function linkHrefAt(target: EventTarget | null): string | null {
  const closest = (target as Element | null)?.closest;
  if (typeof closest !== "function") return null;
  return (target as Element).closest("a[href]")?.getAttribute("href") ?? null;
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
    for (const property of safeStyleProperties) {
      const value = original.getPropertyValue(property).trim();
      const safeValue = sanitizeCssDeclaration(property, value);
      if (safeValue !== null) declarations.push(`${property}: ${safeValue}`);
    }
    // Same URL scheme check as <img src>. Every remote background, like
    // every remote <img>, is always parked behind the blocked-src marker
    // here — resolving it to a real value is the caller's job (see
    // fillResolvedImages), once the native proxy has fetched it.
    const backgroundImageMatch = original.getPropertyValue("background-image").trim().match(backgroundImageUrl);
    if (backgroundImageMatch) {
      const backgroundUrl = backgroundImageMatch[1] ?? backgroundImageMatch[2] ?? backgroundImageMatch[3] ?? "";
      if (safeImageSrc.test(backgroundUrl) && !/["']/.test(backgroundUrl)) {
        element.setAttribute(blockedSrcAttr, backgroundUrl);
      }
    }
    const tag = element.tagName.toLowerCase();
    if (!dimensionAttributeTags.has(tag)) element.removeAttribute("width");
    if (!dimensionAttributeTags.has(tag)) element.removeAttribute("height");

    const width = element.getAttribute("width");
    if (width !== null) {
      const safeWidth = sanitizeHtmlDimension(width, true);
      if (safeWidth === null) element.removeAttribute("width");
      else element.setAttribute("width", safeWidth);
    }
    const height = element.getAttribute("height");
    if (height !== null) {
      const safeHeight = sanitizeHtmlDimension(height, false);
      if (safeHeight === null) element.removeAttribute("height");
      else element.setAttribute("height", safeHeight);
    }
    // Some templates size elements via CSS instead of the width/height
    // attributes above; apply the same bounds either way.
    if (tag === "table") {
      for (const attr of ["cellpadding", "cellspacing"]) {
        const value = element.getAttribute(attr);
        if (value === null) continue;
        const safeValue = sanitizeHtmlDimension(value, false);
        if (safeValue === null) element.removeAttribute(attr);
        else element.setAttribute(attr, safeValue);
      }
    }
    const bgcolor = element.getAttribute("bgcolor");
    if (bgcolor !== null) {
      const value = bgcolor.trim().toLowerCase();
      if (safeColor.test(value)) element.setAttribute("bgcolor", value);
      else element.removeAttribute("bgcolor");
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

  linkifyTextNodes(fragment);

  const container = document.createElement("div");
  container.append(fragment);
  return container.innerHTML;
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
/**
 * Fills resolved images into an already-parsed document (the live message
 * frame) in place. Patching the loaded frame instead of rebuilding its
 * `srcdoc` means each arriving image no longer reloads the whole frame,
 * which flashed the content and reset scrolling and selection. Only `data:`
 * URIs produced by the native proxy/attachment reader are admitted, the
 * same values the frame's `img-src data:` CSP already allowed.
 */
export function fillResolvedImages(root: ParentNode, resolved: ReadonlyMap<string, string>): void {
  if (resolved.size === 0) return;
  root.querySelectorAll<HTMLElement>(`[${blockedSrcAttr}]`).forEach((element) => {
    const url = element.getAttribute(blockedSrcAttr);
    const dataUri = url ? resolved.get(url) : undefined;
    if (!dataUri || !/^data:/i.test(dataUri)) return;
    element.removeAttribute(blockedSrcAttr);
    if (element.tagName.toLowerCase() === "img") {
      element.setAttribute("src", dataUri);
    } else {
      element.style.setProperty("background-image", `url("${dataUri}")`);
    }
  });
}

// Only in-flight work is retained here. The native proxy owns the bounded
// result cache; retaining fulfilled data URIs in JS as well would create a
// second, unbounded cache and another full copy of every image.
const pendingImagePromises = new Map<string, Promise<string>>();
const globalImageQueue: Array<() => void> = [];
let globallyActiveImages = 0;

function withGlobalImageSlot<T>(task: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const start = () => {
      globallyActiveImages += 1;
      // Resolve through a microtask so a synchronously throwing injected
      // resolver cannot leak a global slot.
      void Promise.resolve().then(task).then(resolve, reject).finally(() => {
        globallyActiveImages -= 1;
        globalImageQueue.shift()?.();
      });
    };
    if (globallyActiveImages < EMAIL_IMAGE_LIMITS.maxConcurrentGlobally) start();
    else if (globallyActiveImages + globalImageQueue.length >= EMAIL_IMAGE_LIMITS.maxPendingGlobally) {
      reject(new Error("Global image queue is full"));
    }
    else globalImageQueue.push(start);
  });
}

function resolveSharedImage(cacheKey: string, url: string, resolveImage: (url: string) => Promise<string>) {
  const existing = pendingImagePromises.get(cacheKey);
  if (existing) return existing;
  const pending = withGlobalImageSlot(() => resolveImage(url));
  pendingImagePromises.set(cacheKey, pending);
  void pending.finally(() => {
    if (pendingImagePromises.get(cacheKey) === pending) pendingImagePromises.delete(cacheKey);
  }).catch(() => {
    // The caller handles the original rejection. This catch only consumes the
    // promise returned by finally.
  });
  return pending;
}

export function fitsMessageImageBudget(currentBytes: number, candidateBytes: number): boolean {
  return candidateBytes <= EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage - currentBytes;
}

export function SafeMessage({
  html,
  text = "",
  loadImages = false,
  resolveImage = defaultResolveImage,
  imageCacheKey = "",
  theme = "dark",
  fontScale = 1,
  emailMinimumFontSize = 0,
  fontFamily = "system",
  tone = "default",
  priorThreadText,
  onImageClick,
  onEnterKey,
}: SafeMessageProps) {
  const minimumFontSize = parseEmailMinimumFontSize(emailMinimumFontSize);
  const minimumFontSizeRef = useRef(minimumFontSize);
  minimumFontSizeRef.current = minimumFontSize;
  const fontSizeControllerRef = useRef<((value: number) => void) | null>(null);
  useEffect(() => { fontSizeControllerRef.current?.(minimumFontSize); }, [minimumFontSize]);
  const onImageClickRef = useRef(onImageClick);
  onImageClickRef.current = onImageClick;
  // Callers typically pass an inline resolver; reading it through a ref
  // keeps each parent render from cancelling and restarting image loads.
  const resolveImageRef = useRef(resolveImage);
  resolveImageRef.current = resolveImage;
  const onEnterKeyRef = useRef(onEnterKey);
  onEnterKeyRef.current = onEnterKey;
  const [imagesAllowedForMessage, setImagesAllowedForMessage] = useState(false);
  const [quotedHistoryExpanded, setQuotedHistoryExpanded] = useState(false);
  const imagesAllowed = loadImages || imagesAllowedForMessage;
  const sanitized = useMemo(() => sanitizeMessageHtml(html), [html]);
  const htmlFold = useMemo(() => foldQuotedHistoryHtml(sanitized, priorThreadText), [sanitized, priorThreadText]);
  const textFold = useMemo(() => foldQuotedHistoryText(text, priorThreadText), [text, priorThreadText]);
  const hasCollapsedHistory = htmlFold !== null || (!sanitized.trim() && textFold !== null);
  const renderedHtml = htmlFold ? (quotedHistoryExpanded ? htmlFold.expanded : htmlFold.visible) : sanitized;
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
  const acceptedImageBytesRef = useRef(0);
  const acceptedImageUrlsRef = useRef(new Set<string>());

  useEffect(() => {
    if (resolvableUrls.length === 0) return;
    let cancelled = false;
    let nextIndex = 0;
    const urls = resolvableUrls.slice(0, EMAIL_IMAGE_LIMITS.maxImagesPerMessage);

    const worker = async () => {
      while (!cancelled) {
        const url = urls[nextIndex++];
        if (!url) return;
        if (acceptedImageUrlsRef.current.has(url)) continue;
        const cacheKey = /^cid:/i.test(url) ? `${imageCacheKey}\0${url}` : url;
        try {
          const dataUri = await resolveSharedImage(cacheKey, url, (next) => resolveImageRef.current(next));
          if (cancelled) return;
          // Data URIs are ASCII, so string length is their byte size. Reject
          // the whole resource instead of truncating sender-controlled data.
          if (!fitsMessageImageBudget(acceptedImageBytesRef.current, dataUri.length)) return;
          acceptedImageBytesRef.current += dataUri.length;
          acceptedImageUrlsRef.current.add(url);
          setResolvedImages((previous) => {
            if (previous.get(url) === dataUri) return previous;
            const next = new Map(previous);
            next.set(url, dataUri);
            return next;
          });
        } catch {
          // A failed image stays behind its inert blocked-src marker. Since
          // only in-flight promises are shared, a later retry remains possible.
        }
      }
    };

    const workerCount = Math.min(urls.length, EMAIL_IMAGE_LIMITS.maxConcurrentPerMessage);
    for (let index = 0; index < workerCount; index += 1) void worker();

    return () => {
      cancelled = true;
    };
  }, [imageCacheKey, resolvableUrls]);

  const resolvedImagesRef = useRef(resolvedImages);
  resolvedImagesRef.current = resolvedImages;
  const emailStyleSheet = useMemo(() => extractSafeStyleSheet(html, theme), [html, theme]);

  const doc = useMemo(
    () => buildMessageDocument(renderedHtml, { theme, fontScale, fontFamily, tone, emailStyleSheet }),
    [renderedHtml, theme, fontScale, fontFamily, tone, emailStyleSheet],
  );

  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const [frameHeight, setFrameHeight] = useState(0);
  const [foldTop, setFoldTop] = useState<number | null>(null);
  // The real destination of the link under the pointer or keyboard focus.
  // Shown by the app outside the untrusted document, so a sender's markup,
  // CSS, or title attribute can't disguise where a link actually goes.
  const [hoveredLinkHref, setHoveredLinkHref] = useState<string | null>(null);

  const handleLoad = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;

    const frame = frameRef.current;
    const frameDoc = frame?.contentDocument;
    if (!frameDoc) return;
    const updateFontSize = createEmailFontSizeController(frameDoc);
    fontSizeControllerRef.current = updateFontSize;
    updateFontSize(minimumFontSizeRef.current);
    // A reload (theme change, quote expansion) starts from the blocked
    // markup again, so re-apply everything resolved so far.
    fillResolvedImages(frameDoc, resolvedImagesRef.current);

    const resize = () => {
      const height = frameDoc.documentElement?.scrollHeight ?? frameDoc.body?.scrollHeight ?? 0;
      const nextHeight = emailFrameHeight(height);
      setFrameHeight((previous) => previous === nextHeight ? previous : nextHeight);
      // The toggle lives outside the untrusted document; follow the marker
      // so it stays at the fold whether the quoted part is shown or hidden.
      const marker = frameDoc.querySelector(`span[${QUOTED_HISTORY_FOLD_ATTRIBUTE}]`);
      const nextFoldTop = marker ? Math.round(marker.getBoundingClientRect().top + (frameDoc.defaultView?.scrollY ?? 0)) : null;
      setFoldTop((previous) => previous === nextFoldTop ? previous : nextFoldTop);
    };
    resize();
    const onResize = () => {
      updateFontSize(minimumFontSizeRef.current);
      resize();
    };
    frameDoc.defaultView?.addEventListener("resize", onResize);

    let observer: ResizeObserver | undefined;
    if (frameDoc.body && typeof ResizeObserver !== "undefined") {
      observer = new ResizeObserver(resize);
      observer.observe(frameDoc.body);
    }

    const onClick = (event: MouseEvent) => {
      const target = event.target as HTMLElement | null;
      const link = target?.closest("a");
      const href = link?.getAttribute("href");
      if (href) {
        event.preventDefault();
        void openUrl(href);
        return;
      }
      // Read the live src (not the sanitized markup) so this reflects
      // whatever fillResolvedImages ended up resolving the image to.
      const src = target?.closest("img")?.getAttribute("src");
      if (!src) return;
      event.preventDefault();
      onImageClickRef.current?.(src);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      const plainEnter = event.key === "Enter"
        && !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey;
      if (plainEnter && onEnterKeyRef.current) {
        event.preventDefault();
        onEnterKeyRef.current();
        return;
      }
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
    const onLinkEnter = (event: Event) => setHoveredLinkHref(linkHrefAt(event.target));
    const onLinkLeave = (event: MouseEvent | FocusEvent) => {
      // Moving between children of the same link fires out/over pairs;
      // only clear once the pointer or focus has actually left the link.
      const leaving = linkHrefAt(event.target);
      if (leaving !== null && linkHrefAt(event.relatedTarget) !== leaving) setHoveredLinkHref(null);
    };
    frameDoc.addEventListener("click", onClick);
    frameDoc.addEventListener("keydown", onKeyDown);
    frameDoc.addEventListener("mouseover", onLinkEnter);
    frameDoc.addEventListener("mouseout", onLinkLeave);
    frameDoc.addEventListener("focusin", onLinkEnter);
    frameDoc.addEventListener("focusout", onLinkLeave);

    cleanupRef.current = () => {
      observer?.disconnect();
      frameDoc.defaultView?.removeEventListener("resize", onResize);
      frameDoc.removeEventListener("click", onClick);
      frameDoc.removeEventListener("keydown", onKeyDown);
      frameDoc.removeEventListener("mouseover", onLinkEnter);
      frameDoc.removeEventListener("mouseout", onLinkLeave);
      frameDoc.removeEventListener("focusin", onLinkEnter);
      frameDoc.removeEventListener("focusout", onLinkLeave);
      setHoveredLinkHref(null);
    };
  }, []);

  useEffect(() => () => cleanupRef.current?.(), []);

  useEffect(() => {
    const frameDoc = frameRef.current?.contentDocument;
    if (frameDoc) fillResolvedImages(frameDoc, resolvedImages);
  }, [resolvedImages]);

  useEffect(() => {
    setQuotedHistoryExpanded(false);
  }, [html, text]);

  const quotedHistoryLabel = quotedHistoryExpanded ? "Hide quoted content" : "Show quoted content";
  const renderQuotedHistoryButton = (top: number | null) => hasCollapsedHistory ? (
    <button
      type="button"
      className="quoted-history-toggle"
      aria-label={quotedHistoryLabel}
      aria-expanded={quotedHistoryExpanded}
      title={quotedHistoryLabel}
      style={top === null ? undefined : { position: "absolute", top }}
      onClick={() => setQuotedHistoryExpanded((expanded) => !expanded)}
    >
      <svg viewBox="0 0 16 4" width="16" height="4" aria-hidden="true" focusable="false">
        <circle cx="2" cy="2" r="1.6" />
        <circle cx="8" cy="2" r="1.6" />
        <circle cx="14" cy="2" r="1.6" />
      </svg>
    </button>
  ) : null;

  const linkStatus = hoveredLinkHref ? (
    <div className="message-link-status" data-testid="message-link-status" aria-hidden="true">
      {hoveredLinkHref}
    </div>
  ) : null;

  if (!hasContent) {
    const decoded = decodeHtmlEntities(text);
    const plainFold = !sanitized.trim() ? textFold : null;
    return (
      <div
        className="message-body message-body-plain"
        data-testid="message-body"
        style={minimumFontSize ? { fontSize: `max(${minimumFontSize}px, var(--text-reading))` } : undefined}
        onMouseOver={(event) => setHoveredLinkHref(linkHrefAt(event.target))}
        onMouseOut={(event) => {
          const leaving = linkHrefAt(event.target);
          if (leaving !== null && linkHrefAt(event.relatedTarget) !== leaving) setHoveredLinkHref(null);
        }}
        onFocus={(event) => setHoveredLinkHref(linkHrefAt(event.target))}
        onBlur={(event) => {
          const leaving = linkHrefAt(event.target);
          if (leaving !== null && linkHrefAt(event.relatedTarget) !== leaving) setHoveredLinkHref(null);
        }}
      >
        {plainFold ? (
          <>
            {linkifyText(decodeHtmlEntities(plainFold.visible))}
            {renderQuotedHistoryButton(null)}
            {quotedHistoryExpanded ? linkifyText(decodeHtmlEntities(plainFold.quoted)) : null}
          </>
        ) : decoded ? linkifyText(decoded) : "No message content."}
        {linkStatus}
      </div>
    );
  }

  return (
    <>
      {hasBlockedImages ? (
        <div className="message-images-notice">
          <span>Images are blocked in this message.</span>
          <button type="button" className="btn btn-sm" onClick={() => setImagesAllowedForMessage(true)}>
            <Image size={ICON_SIZE.sm} /> Load images
          </button>
        </div>
      ) : null}
      <div className="message-frame">
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
        {renderQuotedHistoryButton(foldTop)}
        {linkStatus}
      </div>
    </>
  );
}
