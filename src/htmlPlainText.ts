/**
 * Flattens an HTML fragment to readable plain text: <br> and block elements
 * become line breaks, entities are decoded, and script/style content is dropped.
 *
 * Parsing happens in a DOMParser document, which is inert: scripts never run,
 * event handlers never fire, and images or other resources are never fetched.
 * Only the resulting text is returned; no parsed markup reaches the UI.
 */

const BLOCK_ELEMENTS = new Set([
  "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "DD", "DIV", "DL", "DT", "FIGCAPTION", "FIGURE",
  "FOOTER", "H1", "H2", "H3", "H4", "H5", "H6", "HEADER", "HR", "LI", "OL", "P", "PRE", "SECTION",
  "TABLE", "TR", "UL",
]);

const IGNORED_ELEMENTS = new Set(["SCRIPT", "STYLE", "TEMPLATE", "NOSCRIPT", "HEAD", "TITLE"]);

export type HtmlWhitespace =
  /** Source whitespace is HTML formatting: collapse runs and trim around line breaks. */
  | "collapse"
  /** Text-node whitespace is content (for example, an editable round trip): keep it. */
  | "preserve";

export function htmlToPlainText(html: string, { whitespace }: { whitespace: HtmlWhitespace }): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  let text = "";
  const breakLine = () => {
    text = text.replace(/[ \t ]+$/, "");
    if (text && !text.endsWith("\n")) text += "\n";
  };
  const visit = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      const content = node.textContent ?? "";
      text += whitespace === "collapse" ? content.replace(/\s+/g, " ") : content;
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

  if (whitespace === "collapse") {
    text = text.replace(/[ \t ]+\n/g, "\n").replace(/\n[ \t ]+/g, "\n");
  }
  return text.replace(/\n{3,}/g, "\n\n").trim();
}
