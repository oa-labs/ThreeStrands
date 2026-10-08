import DOMPurify from "dompurify";
import { matchesShortcut } from "./commands";
import { sanitizeCssDeclaration } from "./emailRenderingPolicy";
import { LINKIFY_PATTERN, linkHrefFor, trimTrailingPunctuation } from "./linkify";

export type FormattingShortcut = {
  id: string;
  title: string;
  key: string;
  command: string;
  value?: string;
  prompt?: "link" | "color";
  listOnly?: boolean;
};

export const formattingShortcuts: FormattingShortcut[] = [
  { id: "format.bold", title: "Bold", key: "Mod+b", command: "bold" },
  { id: "format.italic", title: "Italics", key: "Mod+i", command: "italic" },
  { id: "format.underline", title: "Underline", key: "Mod+u", command: "underline" },
  { id: "format.link", title: "Hyperlink", key: "Mod+k", command: "createLink", prompt: "link" },
  { id: "format.color", title: "Color", key: "Mod+Shift+c", command: "foreColor", prompt: "color" },
  { id: "format.strike", title: "Strikethrough", key: "Mod+Shift+x", command: "strikeThrough" },
  { id: "format.fixedWidth", title: "Fixed-Width", key: "Mod+Shift+m", command: "fontName", value: "monospace" },
  { id: "format.numbers", title: "Numbered List", key: "Mod+Shift+7", command: "insertOrderedList" },
  { id: "format.bullets", title: "Bulleted List", key: "Mod+Shift+8", command: "insertUnorderedList" },
  { id: "format.quote", title: "Quote", key: "Mod+Shift+9", command: "formatBlock", value: "blockquote" },
  { id: "format.indentList", title: "Indent List", key: "Tab", command: "indent", listOnly: true },
  { id: "format.outdentList", title: "Outdent List", key: "Shift+Tab", command: "outdent", listOnly: true },
  { id: "format.indent", title: "Increase Indent", key: "Mod+]", command: "indent" },
  { id: "format.outdent", title: "Decrease Indent", key: "Mod+[", command: "outdent" },
];

export function formattingShortcutFor(event: KeyboardEvent): FormattingShortcut | undefined {
  return formattingShortcuts.find((shortcut) => matchesShortcut(event, shortcut.key));
}

export function selectionIsInList(editor: HTMLElement): boolean {
  const node = window.getSelection()?.anchorNode;
  const element = node instanceof Element ? node : node?.parentElement;
  return Boolean(element?.closest("li") && editor.contains(element));
}

// Markdown-style markers that start a list when typed alone and followed by
// a space: `*` for bullets, `1.` for numbers.
const listShortcutMarkers: Record<string, "ul" | "ol"> = { "*": "ul", "1.": "ol" };

/**
 * Converts a standalone list marker (`*` or `1.`) followed by a space into an
 * empty bulleted or numbered list. The caller invokes this while handling the
 * space key, before the browser inserts the space itself.
 */
export function applyListShortcut(editor: HTMLElement): boolean {
  const selection = window.getSelection();
  if (!selection?.rangeCount || !selection.isCollapsed) return false;

  const range = selection.getRangeAt(0);
  if (!editor.contains(range.endContainer)) return false;

  const anchorElement = range.endContainer instanceof Element
    ? range.endContainer
    : range.endContainer.parentElement;
  if (anchorElement?.closest("li")) return false;

  const block = anchorElement?.closest("p, div, blockquote, pre");
  const container = block && block !== editor && editor.contains(block)
    ? block
    : Object.hasOwn(listShortcutMarkers, editor.textContent ?? "") && !editor.querySelector("p, div, blockquote, pre, br")
      ? editor
      : null;
  if (!container) return false;

  const contents = document.createRange();
  contents.selectNodeContents(container);
  const beforeCaret = contents.cloneRange();
  beforeCaret.setEnd(range.endContainer, range.endOffset);
  const afterCaret = contents.cloneRange();
  afterCaret.setStart(range.endContainer, range.endOffset);
  const marker = beforeCaret.toString();
  if (!Object.hasOwn(listShortcutMarkers, marker) || afterCaret.toString() !== "") return false;

  const list = document.createElement(listShortcutMarkers[marker]);
  const item = document.createElement("li");
  item.append(document.createElement("br"));
  list.append(item);
  if (container === editor) editor.replaceChildren(list);
  else container.replaceWith(list);

  const caret = document.createRange();
  caret.setStart(item, 0);
  caret.collapse(true);
  selection.removeAllRanges();
  selection.addRange(caret);
  return true;
}

/**
 * The composer's fixed-width face, applied with `fontName` and removed by the
 * same shortcut again — WebKit's `fontName` implementation never toggles off
 * on its own, so the undo path is ours to provide.
 */
const fixedWidthFamily = "monospace";

function isFixedWidthMarked(element: Element): boolean {
  if (element instanceof HTMLFontElement) {
    const face = element.getAttribute("face");
    return face !== null && sanitizeCssDeclaration("font-family", face) === fixedWidthFamily;
  }
  return element instanceof HTMLElement
    && sanitizeCssDeclaration("font-family", element.style.fontFamily) === fixedWidthFamily;
}

function fixedWidthAnchorElement(editor: HTMLElement): Element | null {
  const node = window.getSelection()?.anchorNode;
  const element = node instanceof Element ? node : node?.parentElement;
  return element && editor.contains(element) ? element : null;
}

/** True when the selection's anchor sits in text explicitly marked fixed-width. */
export function selectionIsFixedWidth(editor: HTMLElement): boolean {
  let element = fixedWidthAnchorElement(editor);
  while (element && element !== editor) {
    if (isFixedWidthMarked(element)) return true;
    element = element.parentElement;
  }
  return false;
}

function stripFixedWidth(editor: HTMLElement): void {
  let element = fixedWidthAnchorElement(editor);
  while (element && element !== editor) {
    const parent = element.parentElement;
    if (isFixedWidthMarked(element)) {
      if (element instanceof HTMLFontElement) element.removeAttribute("face");
      else if (element instanceof HTMLElement) {
        element.style.removeProperty("font-family");
        if (element.getAttribute("style") === "") element.removeAttribute("style");
      }
      // A span that carried only the fixed-width family was structure we
      // added; unwrap it so repeated toggles don't nest empty spans.
      if (element instanceof HTMLSpanElement && !element.attributes.length) {
        element.replaceWith(...element.childNodes);
      }
    }
    element = parent;
  }
}

export function applyFormattingShortcut(
  editor: HTMLElement,
  shortcut: FormattingShortcut,
  ask: typeof window.prompt = window.prompt,
): boolean {
  if (shortcut.listOnly && !selectionIsInList(editor)) return false;

  let value = shortcut.value ?? "";
  if (shortcut.prompt === "link") {
    const input = ask("Link URL", "https://")?.trim();
    if (!input) return true;
    value = /^(https?:\/\/|mailto:|tel:)/i.test(input) ? input : `https://${input}`;
  } else if (shortcut.prompt === "color") {
    const input = ask("Text color", "#2563eb")?.trim();
    if (!input) return true;
    value = input;
  }

  editor.focus();
  if (shortcut.command === "fontName" && selectionIsFixedWidth(editor)) {
    stripFixedWidth(editor);
    return true;
  }
  document.execCommand(shortcut.command, false, value);
  return true;
}

export function plainTextToHtml(text: string): string {
  const container = document.createElement("div");
  text.split("\n").forEach((line, index) => {
    if (index) container.append(document.createElement("br"));
    container.append(document.createTextNode(line));
  });
  return container.innerHTML;
}

const replyAttributionLine = /^On .+ wrote:$/;

/**
 * Inline style ThreeStrands writes on a reply's citation so recipients that do
 * not style `<blockquote type="cite">` themselves still show a quote bar. It is
 * a fixed value set by the compose sanitizer, never copied from content.
 */
const CITATION_STYLE = "margin: 0px 0px 0px 0.8ex; border-left: 1px solid rgb(204, 204, 204); padding-left: 1ex;";

function appendCitation(parent: Element, lines: string[]) {
  const quote = document.createElement("blockquote");
  quote.setAttribute("type", "cite");
  let continuesLine = false;
  for (let index = 0; index < lines.length;) {
    if (lines[index].startsWith(">")) {
      const nested: string[] = [];
      while (index < lines.length && lines[index].startsWith(">")) nested.push(lines[index++].replace(/^> ?/, ""));
      appendCitation(quote, nested);
      continuesLine = false;
      continue;
    }
    if (continuesLine) quote.append(document.createElement("br"));
    quote.append(document.createTextNode(lines[index++]));
    continuesLine = true;
  }
  parent.append(quote);
}

/**
 * Converts a native draft's text body to compose HTML. A reply body — an
 * "On … wrote:" attribution followed only by `>`-quoted lines — becomes a real
 * `<blockquote type="cite">` (nested for deeper `>` levels) so recipients'
 * mail clients, and ThreeStrands' own reader, recognize and fold the quoted
 * history. Any other text converts line for line.
 */
export function draftTextToComposeHtml(text: string): string {
  const lines = text.split("\n");
  const attribution = lines.findIndex((line) => replyAttributionLine.test(line));
  const quoted = attribution >= 0 ? lines.slice(attribution + 1) : [];
  if (!quoted.length || !quoted.every((line) => line.startsWith(">"))) return plainTextToHtml(text);
  const container = document.createElement("div");
  container.innerHTML = plainTextToHtml(lines.slice(0, attribution + 1).join("\n"));
  appendCitation(container, quoted.map((line) => line.replace(/^> ?/, "")));
  return container.innerHTML;
}

const TEXT_BLOCK_ELEMENTS = new Set(["BLOCKQUOTE", "DIV", "LI", "OL", "P", "UL"]);

function isBlankTrailingNode(node: Node): boolean {
  return (node.nodeType === Node.TEXT_NODE && !(node as Text).data.trim())
    || (node instanceof Element && node.tagName === "BR");
}

/**
 * Splits compose HTML into the part the user writes and the quoted history a
 * reply starts with, so the composer can keep the history out of the editor
 * the user types in. Assistive and dictation software reads the focused
 * editor's entire text, which stalls on a long quoted thread.
 *
 * Only a body that ends with an "On … wrote:" attribution followed by a
 * `<blockquote type="cite">` splits; anything after the citation (an inline
 * answer) or a citation without its attribution leaves the body whole.
 */
export function splitReplyQuote(html: string): { authoredHtml: string; quotedHtml: string } | null {
  const template = document.createElement("template");
  template.innerHTML = html;
  const root = template.content;
  let citation = root.lastChild;
  while (citation && isBlankTrailingNode(citation)) citation = citation.previousSibling;
  if (!(citation instanceof Element) || citation.tagName !== "BLOCKQUOTE" || citation.getAttribute("type")?.toLowerCase() !== "cite") return null;

  // The attribution is either the inline run since the last line break, or
  // one block element when editing wrapped the line.
  let start: ChildNode | null = citation.previousSibling;
  if (start instanceof Element && TEXT_BLOCK_ELEMENTS.has(start.tagName)) {
    if (!replyAttributionLine.test(start.textContent?.trim() ?? "")) return null;
  } else {
    let attribution = "";
    let node: ChildNode | null = start;
    while (node && !(node instanceof Element && (node.tagName === "BR" || TEXT_BLOCK_ELEMENTS.has(node.tagName)))) {
      attribution = `${node.textContent ?? ""}${attribution}`;
      start = node;
      node = node.previousSibling;
    }
    if (!start || start === citation || !replyAttributionLine.test(attribution.trim())) return null;
  }

  const authored = document.createElement("div");
  const quoted = document.createElement("div");
  while (root.firstChild && root.firstChild !== start) authored.append(root.firstChild);
  while (root.firstChild) quoted.append(root.firstChild);
  while (authored.lastChild && isBlankTrailingNode(authored.lastChild)) authored.lastChild.remove();
  return { authoredHtml: authored.innerHTML, quotedHtml: quoted.innerHTML };
}

/**
 * Plain-text alternative for compose HTML. `innerText` cannot be used: the
 * editor is serialized from a detached clone, where engines fall back to
 * `textContent` and drop the line breaks that blocks and (in Chromium) `<br>`
 * render. Blocks and `<br>` break lines the way the editor shows them, and
 * each blockquote level prefixes its lines with "> ".
 */
export function composeHtmlToText(root: Node): string {
  let text = "";
  let pendingBreak = false;
  const emit = (value: string) => {
    if (pendingBreak && text && !text.endsWith("\n")) text += "\n";
    pendingBreak = false;
    text += value;
  };
  root.childNodes.forEach(function visit(node: Node) {
    if (node.nodeType === Node.TEXT_NODE) {
      const data = (node as Text).data;
      if (data) emit(data.replace(/\u00a0/g, " "));
      return;
    }
    if (!(node instanceof Element)) return;
    if (node.tagName === "BR") {
      emit("\n");
      return;
    }
    if (node.tagName === "BLOCKQUOTE") {
      const inner = composeHtmlToText(node).replace(/\n+$/, "");
      pendingBreak = true;
      emit(inner.split("\n").map((line) => (line ? `> ${line}` : ">")).join("\n"));
      pendingBreak = true;
      return;
    }
    const block = TEXT_BLOCK_ELEMENTS.has(node.tagName);
    if (block) pendingBreak = true;
    node.childNodes.forEach(visit);
    if (block) pendingBreak = true;
  });
  return text;
}

/** Turns bare URLs/emails pasted into the composer into real <a> tags. */
/**
 * The link destination for pasted text that is exactly one URL or email
 * address (surrounding whitespace aside), or null when the text is anything
 * else. Pasting such text over a selection links the selection instead of
 * replacing it.
 */
export function pastedLinkHref(text: string): string | null {
  const candidate = text.trim();
  const match = candidate.match(new RegExp(`^${LINKIFY_PATTERN.source}$`, "i"));
  if (!match) return null;
  const { url } = trimTrailingPunctuation(candidate);
  return url ? linkHrefFor(url) : null;
}

export function linkifyPlainText(text: string): string {
  const container = document.createElement("div");
  text.split("\n").forEach((line, lineIndex) => {
    if (lineIndex) container.append(document.createElement("br"));
    let lastIndex = 0;
    for (const match of line.matchAll(LINKIFY_PATTERN)) {
      const index = match.index ?? 0;
      const { url, trailing } = trimTrailingPunctuation(match[0]);
      if (!url) continue;
      if (index > lastIndex) container.append(document.createTextNode(line.slice(lastIndex, index)));
      const anchor = document.createElement("a");
      anchor.setAttribute("href", linkHrefFor(url));
      anchor.textContent = url;
      container.append(anchor);
      lastIndex = index + match[0].length;
      if (trailing) container.append(document.createTextNode(trailing));
    }
    if (lastIndex < line.length) container.append(document.createTextNode(line.slice(lastIndex)));
  });
  return container.innerHTML;
}

export function sanitizeComposeHtml(html: string): string {
  const clean = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: ["a", "b", "blockquote", "br", "div", "em", "font", "i", "img", "li", "ol", "p", "span", "strike", "strong", "u", "ul"],
    ALLOWED_ATTR: ["alt", "color", "face", "href", "src", "style", "type", "width"],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
  });
  const container = document.createElement("div");
  container.innerHTML = clean;
  // Legacy <font face> is the older HTML spelling of font-family, and what
  // WebKit's `fontName` produces live. Normalize every face through the
  // shared CSS policy: valid families move to inline style (which the reader
  // already renders), invalid ones are dropped while the element's content
  // and other attributes survive.
  container.querySelectorAll("font[face]").forEach((font) => {
    const family = sanitizeCssDeclaration("font-family", font.getAttribute("face") ?? "");
    const span = document.createElement("span");
    if (family) span.style.fontFamily = family;
    for (const attribute of Array.from(font.attributes)) {
      if (attribute.name !== "face") span.setAttribute(attribute.name, attribute.value);
    }
    while (font.firstChild) span.append(font.firstChild);
    font.replaceWith(span);
  });
  container.querySelectorAll<HTMLElement>("[style]").forEach((element) => {
    const margin = element.style.marginLeft;
    const family = element.style.fontFamily;
    element.removeAttribute("style");
    if (/^\d+(\.\d+)?px$/.test(margin)) {
      element.style.marginLeft = `${Math.min(parseFloat(margin), 320)}px`;
    }
    // A font family cannot fetch resources, execute code, or escape the
    // compose surface; the shared policy grammar bounds what counts as one.
    const safeFamily = sanitizeCssDeclaration("font-family", family);
    if (safeFamily) element.style.fontFamily = safeFamily;
  });
  // `type` exists only to mark a reply citation. A citation gets ThreeStrands'
  // fixed quote-bar style in place of whatever style it carried.
  container.querySelectorAll<HTMLElement>("[type]").forEach((element) => {
    if (element.tagName !== "BLOCKQUOTE" || element.getAttribute("type")?.toLowerCase() !== "cite") {
      element.removeAttribute("type");
      return;
    }
    element.setAttribute("type", "cite");
    element.setAttribute("style", CITATION_STYLE);
  });
  container.querySelectorAll<HTMLAnchorElement>("a").forEach((link) => {
    if (!/^(https?:\/\/|mailto:|tel:)/i.test(link.getAttribute("href") ?? "")) {
      link.removeAttribute("href");
    }
  });
  container.querySelectorAll<HTMLImageElement>("img").forEach((image) => {
    const source = image.getAttribute("src") ?? "";
    if (!/^data:image\/(?:avif|gif|jpe?g|png|webp);base64,[a-z0-9+/]+=*$/i.test(source)
        && !/^cid:[a-z0-9._@-]+$/i.test(source)) {
      image.remove();
      return;
    }
    const width = Number.parseInt(image.getAttribute("width") ?? "", 10);
    if (Number.isFinite(width)) image.setAttribute("width", String(Math.min(Math.max(width, 80), 2000)));
    else image.removeAttribute("width");
    image.removeAttribute("style");
    if (!image.getAttribute("alt")) image.setAttribute("alt", "Pasted image");
  });
  return container.innerHTML;
}

/**
 * Inserts an HTML string at a `Range`, then collapses the selection to just
 * after the inserted content. Used instead of `execCommand("insertHTML", …)`
 * when the range was captured before focus moved elsewhere (e.g. into a
 * modal) and then restored — some WebKit-based webviews normalize or drop a
 * programmatically restored selection on focus, so `execCommand` can insert
 * nothing even though the range object itself is still valid.
 */
export function insertHtmlAtRange(editor: HTMLElement, range: Range, html: string): void {
  const template = document.createElement("template");
  template.innerHTML = html;
  const fragment = template.content;
  const lastNode = fragment.lastChild;
  range.deleteContents();
  range.insertNode(fragment);
  const selection = window.getSelection();
  if (!selection) return;
  const after = document.createRange();
  if (lastNode && editor.contains(lastNode)) after.setStartAfter(lastNode);
  else after.selectNodeContents(editor);
  after.collapse(false);
  selection.removeAllRanges();
  selection.addRange(after);
}

/**
 * Removes compose-only image controls and returns the draft's HTML and plain
 * text. Both come from a single clone: the body includes the quoted thread,
 * so every extra copy adds autosave cost proportional to the whole thread.
 *
 * A reply's separately edited quoted history (see `splitReplyQuote`) joins
 * the authored text after a blank line, the shape a native reply starts with.
 */
export function serializeComposeBody(editor: HTMLElement, quoted?: HTMLElement | null): { html: string; text: string } {
  const clone = editor.cloneNode(true) as HTMLElement;
  if (quoted && (quoted.textContent?.trim() || quoted.querySelector("img"))) {
    while (clone.lastChild && isBlankTrailingNode(clone.lastChild)) clone.lastChild.remove();
    clone.append(document.createElement("br"), document.createElement("br"), ...Array.from(quoted.cloneNode(true).childNodes));
  }
  clone.querySelectorAll<HTMLElement>("[data-compose-image]").forEach((wrapper) => {
    const image = wrapper.querySelector("img");
    const composeSource = image?.dataset.composeSource;
    if (image && composeSource) image.setAttribute("src", composeSource);
    if (image) wrapper.replaceWith(image);
    else wrapper.remove();
  });
  return {
    html: sanitizeComposeHtml(clone.innerHTML),
    text: composeHtmlToText(clone),
  };
}

/** Removes compose-only image controls before a draft is saved or sent. */
export function serializeComposeHtml(editor: HTMLElement): string {
  return serializeComposeBody(editor).html;
}
