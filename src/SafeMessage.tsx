import DOMPurify from "dompurify";
import { useMemo } from "react";

type SafeMessageProps = {
  html: string;
  text?: string;
};

const allowedTags = [
  "a", "b", "blockquote", "br", "caption", "code", "col", "colgroup",
  "dd", "del", "div", "dl", "dt", "em", "h1", "h2", "h3", "h4",
  "h5", "h6", "hr", "i", "kbd", "li", "ol", "p", "pre", "s",
  "small", "span", "strong", "sub", "sup", "table", "tbody", "td",
  "tfoot", "th", "thead", "tr", "u", "ul",
];

// Keep text formatting without allowing positioning, hidden content, or CSS
// network requests. Colors inherit the reader's dark theme for legibility.
const safeStyles: Record<string, RegExp> = {
  "text-align": /^(left|right|center|justify|start|end)$/,
  "font-weight": /^(normal|bold|[1-9]00)$/,
  "font-style": /^(normal|italic|oblique)$/,
  "text-decoration": /^(none|underline|line-through)( (underline|line-through))?$/,
  "vertical-align": /^(baseline|top|middle|bottom|sub|super|text-top|text-bottom)$/,
  "border-collapse": /^(collapse|separate)$/,
};

export function sanitizeMessageHtml(html: string): string {
  const fragment = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: allowedTags,
    ALLOWED_ATTR: ["align", "colspan", "dir", "href", "rowspan", "start", "style", "title", "valign"],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
    FORBID_TAGS: ["form", "img", "script", "style", "svg"],
    RETURN_DOM_FRAGMENT: true,
  });

  fragment.querySelectorAll<HTMLElement>("*").forEach((element) => {
    const original = element.style;
    const declarations: string[] = [];
    for (const [property, pattern] of Object.entries(safeStyles)) {
      const value = original.getPropertyValue(property).trim().toLowerCase();
      if (pattern.test(value)) declarations.push(`${property}: ${value}`);
    }
    // Cap sender spacing so malformed newsletters cannot create huge gaps.
    for (const property of ["padding-top", "padding-right", "padding-bottom", "padding-left", "margin-top", "margin-bottom"]) {
      const value = original.getPropertyValue(property).trim();
      if (/^\d+(\.\d+)?px$/.test(value)) {
        declarations.push(`${property}: ${Math.min(parseFloat(value), 32)}px`);
      }
    }
    element.removeAttribute("style");
    if (declarations.length) element.setAttribute("style", declarations.join("; "));
    if (element.tagName === "A") {
      const href = element.getAttribute("href") ?? "";
      if (/^(https?:\/\/|mailto:|tel:)/i.test(href)) {
        element.setAttribute("target", "_blank");
        element.setAttribute("rel", "noopener noreferrer");
      } else {
        element.removeAttribute("href");
      }
    }
  });
  const container = document.createElement("div");
  container.append(fragment);
  return container.innerHTML;
}

export function SafeMessage({ html, text = "" }: SafeMessageProps) {
  const sanitized = useMemo(() => sanitizeMessageHtml(html), [html]);
  const hasContent = useMemo(() => {
    const container = document.createElement("div");
    container.innerHTML = sanitized;
    return Boolean(container.textContent?.trim());
  }, [sanitized]);

  if (!hasContent) {
    return <div className="message-body message-body-plain" data-testid="message-body">{text || "No message content."}</div>;
  }
  return <div className="message-body" data-testid="message-body" dangerouslySetInnerHTML={{ __html: sanitized }} />;
}
