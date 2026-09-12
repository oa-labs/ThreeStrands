import DOMPurify from "dompurify";
import { Image } from "lucide-react";
import { useMemo, useState } from "react";

type SafeMessageProps = {
  html: string;
  text?: string;
};

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

// Keep text formatting without allowing positioning, hidden content, or CSS
// network requests. Colors inherit the reader's active theme for legibility.
const safeStyles: Record<string, RegExp> = {
  "text-align": /^(left|right|center|justify|start|end)$/,
  "font-weight": /^(normal|bold|[1-9]00)$/,
  "font-style": /^(normal|italic|oblique)$/,
  "text-decoration": /^(none|underline|line-through)( (underline|line-through))?$/,
  "vertical-align": /^(baseline|top|middle|bottom|sub|super|text-top|text-bottom)$/,
  "border-collapse": /^(collapse|separate)$/,
};

const safeImageSrc = /^(https?:|data:image\/)/i;
const blockedSrcAttr = "data-blocked-src";

function isBlank(element: Element): boolean {
  return (element.textContent ?? "").replace(/\s+/g, "") === "";
}

export function sanitizeMessageHtml(html: string, options: { allowImages?: boolean } = {}): string {
  const { allowImages = false } = options;
  const fragment = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: allowedTags,
    ALLOWED_ATTR: ["align", "alt", "colspan", "dir", "href", "rowspan", "src", "start", "style", "title", "valign"],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    FORBID_TAGS: ["form", "script", "style", "svg"],
    RETURN_DOM_FRAGMENT: true,
  });

  fragment.querySelectorAll<HTMLElement>("*").forEach((element) => {
    const original = element.style;
    const declarations: string[] = [];
    for (const [property, pattern] of Object.entries(safeStyles)) {
      const value = original.getPropertyValue(property).trim().toLowerCase();
      if (pattern.test(value)) declarations.push(`${property}: ${value}`);
    }
    const tag = element.tagName.toLowerCase();
    // Empty spacer cells carry no content, so zero their spacing outright
    // instead of just capping it, rather than let it render as a dead gap.
    const isEmptyCell = (tag === "td" || tag === "th") && element.children.length === 0 && isBlank(element);
    for (const property of ["padding-top", "padding-right", "padding-bottom", "padding-left", "margin-top", "margin-bottom"]) {
      const value = original.getPropertyValue(property).trim();
      if (/^\d+(\.\d+)?px$/.test(value)) {
        declarations.push(`${property}: ${isEmptyCell ? 0 : Math.min(parseFloat(value), 32)}px`);
      }
    }
    element.removeAttribute("style");
    if (declarations.length) element.setAttribute("style", declarations.join("; "));

    if (tag === "a") {
      const href = element.getAttribute("href") ?? "";
      if (/^(https?:\/\/|mailto:|tel:)/i.test(href)) {
        element.setAttribute("target", "_blank");
        element.setAttribute("rel", "noopener noreferrer");
      } else {
        element.removeAttribute("href");
      }
    }

    if (tag === "img") {
      const src = element.getAttribute("src") ?? "";
      element.removeAttribute("src");
      if (!safeImageSrc.test(src)) {
        element.remove();
      } else if (allowImages) {
        element.setAttribute("src", src);
      } else {
        element.setAttribute(blockedSrcAttr, src);
      }
    }
  });

  // Now that invalid images are gone and styles are resolved, remove any
  // block-level spacer elements left with no text and no remaining children.
  // Processed in reverse document order so children are handled before their
  // ancestors, letting nested spacer stacks collapse in one pass.
  for (const element of Array.from(fragment.querySelectorAll<HTMLElement>("*")).reverse()) {
    if (!fragment.contains(element)) continue;
    if (spacerTags.has(element.tagName.toLowerCase()) && element.children.length === 0 && isBlank(element)) {
      element.remove();
    }
  }

  const container = document.createElement("div");
  container.append(fragment);
  return container.innerHTML;
}

export function SafeMessage({ html, text = "" }: SafeMessageProps) {
  const [imagesAllowed, setImagesAllowed] = useState(false);
  const sanitized = useMemo(() => sanitizeMessageHtml(html, { allowImages: imagesAllowed }), [html, imagesAllowed]);
  const hasContent = useMemo(() => {
    const container = document.createElement("div");
    container.innerHTML = sanitized;
    return Boolean(container.textContent?.trim()) || container.querySelector("img") !== null;
  }, [sanitized]);
  const hasBlockedImages = !imagesAllowed && sanitized.includes(blockedSrcAttr);

  if (!hasContent) {
    return <div className="message-body message-body-plain" data-testid="message-body">{text || "No message content."}</div>;
  }

  return (
    <>
      {hasBlockedImages ? (
        <div className="message-images-notice">
          <span>Images are blocked in this message.</span>
          <button type="button" onClick={() => setImagesAllowed(true)}>
            <Image size={14} /> Load images
          </button>
        </div>
      ) : null}
      <div className="message-body" data-testid="message-body" dangerouslySetInnerHTML={{ __html: sanitized }} />
    </>
  );
}
