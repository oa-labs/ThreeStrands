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

/**
 * Converts a standalone asterisk followed by a space into an empty bullet.
 * The caller invokes this while handling the space key, before the browser
 * inserts the space itself.
 */
export function applyAsteriskListShortcut(editor: HTMLElement): boolean {
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
    : editor.textContent === "*" && !editor.querySelector("p, div, blockquote, pre, br")
      ? editor
      : null;
  if (!container) return false;

  const contents = document.createRange();
  contents.selectNodeContents(container);
  const beforeCaret = contents.cloneRange();
  beforeCaret.setEnd(range.endContainer, range.endOffset);
  const afterCaret = contents.cloneRange();
  afterCaret.setStart(range.endContainer, range.endOffset);
  if (beforeCaret.toString() !== "*" || afterCaret.toString() !== "") return false;

  const list = document.createElement("ul");
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

/** Turns bare URLs/emails pasted into the composer into real <a> tags. */
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
    ALLOWED_ATTR: ["alt", "color", "face", "href", "src", "style", "width"],
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

/** Removes compose-only image controls before a draft is saved or sent. */
export function serializeComposeHtml(editor: HTMLElement): string {
  const clone = editor.cloneNode(true) as HTMLElement;
  clone.querySelectorAll<HTMLElement>("[data-compose-image]").forEach((wrapper) => {
    const image = wrapper.querySelector("img");
    const composeSource = image?.dataset.composeSource;
    if (image && composeSource) image.setAttribute("src", composeSource);
    if (image) wrapper.replaceWith(image);
    else wrapper.remove();
  });
  return sanitizeComposeHtml(clone.innerHTML);
}
