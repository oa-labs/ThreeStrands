import DOMPurify from "dompurify";

type SafeMessageProps = {
  html: string;
};

const allowedTags = [
  "a",
  "b",
  "blockquote",
  "br",
  "code",
  "div",
  "em",
  "h1",
  "h2",
  "h3",
  "hr",
  "i",
  "kbd",
  "li",
  "ol",
  "p",
  "pre",
  "span",
  "strong",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "u",
  "ul",
];

export function sanitizeMessageHtml(html: string): string {
  return DOMPurify.sanitize(html, {
    ALLOWED_TAGS: allowedTags,
    ALLOWED_ATTR: ["class", "colspan", "href", "rel", "rowspan", "target", "title"],
    ALLOW_DATA_ATTR: false,
    FORBID_TAGS: ["form", "img", "script", "style", "svg"],
    FORBID_ATTR: ["style"],
  });
}

export function SafeMessage({ html }: SafeMessageProps) {
  const sanitized = sanitizeMessageHtml(html);
  return (
    <div
      className="message-body"
      data-testid="message-body"
      dangerouslySetInnerHTML={{ __html: sanitized }}
    />
  );
}
