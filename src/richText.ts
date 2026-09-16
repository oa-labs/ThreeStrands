import DOMPurify from "dompurify";
import { matchesShortcut } from "./commands";
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
  { id: "format.numbers", title: "Numbered list", key: "Mod+Shift+7", command: "insertOrderedList" },
  { id: "format.bullets", title: "Bulleted list", key: "Mod+Shift+8", command: "insertUnorderedList" },
  { id: "format.quote", title: "Quote", key: "Mod+Shift+9", command: "formatBlock", value: "blockquote" },
  { id: "format.indentList", title: "Indent list", key: "Tab", command: "indent", listOnly: true },
  { id: "format.outdentList", title: "Outdent list", key: "Shift+Tab", command: "outdent", listOnly: true },
  { id: "format.indent", title: "Increase indent", key: "Mod+]", command: "indent" },
  { id: "format.outdent", title: "Decrease indent", key: "Mod+[", command: "outdent" },
];

export function formattingShortcutFor(event: KeyboardEvent): FormattingShortcut | undefined {
  return formattingShortcuts.find((shortcut) => matchesShortcut(event, shortcut.key));
}

export function selectionIsInList(editor: HTMLElement): boolean {
  const node = window.getSelection()?.anchorNode;
  const element = node instanceof Element ? node : node?.parentElement;
  return Boolean(element?.closest("li") && editor.contains(element));
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
    ALLOWED_ATTR: ["alt", "color", "href", "src", "style", "width"],
    ALLOW_DATA_ATTR: false,
    ALLOW_ARIA_ATTR: false,
  });
  const container = document.createElement("div");
  container.innerHTML = clean;
  container.querySelectorAll<HTMLElement>("[style]").forEach((element) => {
    const margin = element.style.marginLeft;
    element.removeAttribute("style");
    if (/^\d+(\.\d+)?px$/.test(margin)) {
      element.style.marginLeft = `${Math.min(parseFloat(margin), 320)}px`;
    }
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
