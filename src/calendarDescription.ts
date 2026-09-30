/**
 * Calendar providers often store event descriptions as HTML fragments
 * ("Dial in<br/>Phone: ..."). Event cards render descriptions as plain text,
 * so markup is flattened to readable text instead of showing literal tags.
 *
 * Parsing happens in a DOMParser document, which is inert: scripts never run,
 * event handlers never fire, and images or other resources are never fetched.
 * Only the resulting text reaches the UI; no parsed markup is rendered.
 */

// Only descriptions that contain something tag-shaped are treated as HTML, so
// plain-text descriptions like "Budget < $5k" or "R&D sync" stay verbatim.
const HTML_MARKUP = /<\/?[a-z][a-z0-9-]*(?:\s[^<>]*)?\/?>|&(?:#\d+|#x[0-9a-f]+|[a-z]+);/i;

const BLOCK_ELEMENTS = new Set([
  "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "DD", "DIV", "DL", "DT", "FIGCAPTION", "FIGURE",
  "FOOTER", "H1", "H2", "H3", "H4", "H5", "H6", "HEADER", "HR", "LI", "OL", "P", "PRE", "SECTION",
  "TABLE", "TR", "UL",
]);

const IGNORED_ELEMENTS = new Set(["SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "HEAD", "TITLE"]);

export function calendarDescriptionText(description: string): string {
  if (!HTML_MARKUP.test(description)) return description.trim();

  const doc = new DOMParser().parseFromString(description, "text/html");
  let text = "";
  const breakLine = () => {
    text = text.replace(/[ \t\u00a0]+$/, "");
    if (text && !text.endsWith("\n")) text += "\n";
  };
  const visit = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      // Source newlines inside HTML are formatting, not content.
      text += (node.textContent ?? "").replace(/\s+/g, " ");
      return;
    }
    if (!(node instanceof Element)) return;
    if (IGNORED_ELEMENTS.has(node.tagName)) return;
    if (node.tagName === "BR") {
      text += "\n";
      return;
    }
    const block = BLOCK_ELEMENTS.has(node.tagName);
    if (block) breakLine();
    node.childNodes.forEach(visit);
    if (block) breakLine();
  };
  doc.body.childNodes.forEach(visit);

  return text
    .replace(/[ \t ]+\n/g, "\n")
    .replace(/\n[ \t ]+/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}
