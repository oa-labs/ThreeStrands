import DOMPurify from "dompurify";
import type { Draft } from "./correspondence";
import { composeHtmlToText } from "./richText";
import { sanitizeCssDeclaration, sanitizeHtmlCellPadding, sanitizeHtmlDimension, SIGNATURE_LIMITS, SIGNATURE_FONT_SIZES } from "./emailRenderingPolicy";

export type GmailSignature = { address: string; html: string };
export type Signature = { id: string; name: string; html: string; text: string };
export type SignatureDefaults = { id: string; account: string; address: string; newSignatureId: string | null; replySignatureId: string | null };
export type SignatureLibrary = { signatures: Signature[]; defaults: SignatureDefaults[] };
export type DraftSignature = Omit<Signature, "id"> & { id: string | null; automatic: boolean; htmlOffset: number; textOffset: number };
export type SignatureState = SignatureLibrary & {
  refresh(): Promise<void>;
  save(signature: Signature): Promise<void>;
  remove(id: string): Promise<void>;
  saveDefaults(defaults: SignatureDefaults): Promise<void>;
  inheritDefaults(account: string, address: string): Promise<void>;
};
export const EMPTY_SIGNATURE_LIBRARY: SignatureLibrary = { signatures: [], defaults: [] };

export function signatureDefaultsKey(account: string, address = ""): string {
  return JSON.stringify([account.trim().toLowerCase(), address.trim().toLowerCase()]);
}

export function defaultSignature(library: SignatureLibrary, account: string, address: string, mode: Draft["mode"]): DraftSignature | null {
  const defaults = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account, address))
    ?? library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account));
  const id = mode === "new" ? defaults?.newSignatureId : defaults?.replySignatureId;
  const signature = library.signatures.find((s) => s.id === id);
  return signature ? { ...signature, automatic: true, htmlOffset: 0, textOffset: 0 } : null;
}

// Complete typography, box model, table, flex/flow and cosmetic families.
// Values use the reader's bounded grammar. No URL, position, event handler,
// stylesheet, form or active document is admitted to the editable app DOM;
// these declarations cannot fetch, execute, cover app controls or escape it.
const SIGNATURE_CSS = new Set([
  "color", "font", "font-family", "font-size", "font-style", "font-weight", "font-variant", "line-height", "letter-spacing",
  "text-align", "text-decoration", "text-transform", "text-indent", "white-space", "word-break", "overflow-wrap", "vertical-align",
  "width", "height", "min-width", "max-width", "min-height", "max-height", "box-sizing",
  "margin", "margin-top", "margin-right", "margin-bottom", "margin-left", "padding", "padding-top", "padding-right", "padding-bottom", "padding-left",
  "border", "border-top", "border-right", "border-bottom", "border-left", "border-color", "border-style", "border-width", "border-radius",
  "border-collapse", "border-spacing", "table-layout", "caption-side", "empty-cells",
  "display", "float", "clear", "flex", "flex-direction", "flex-wrap", "flex-grow", "flex-shrink", "flex-basis",
  "align-items", "align-self", "align-content", "justify-content", "gap", "row-gap", "column-gap",
  "list-style", "list-style-type", "list-style-position", "list-style-image", "background-color", "box-shadow",
]);

export function sanitizeSignatureHtml(html: string, allowCid = false): string {
  const container = document.createElement("div");
  container.innerHTML = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: ["a", "b", "blockquote", "br", "div", "em", "font", "hr", "i", "img", "li", "ol", "p", "s", "span", "strike", "strong", "u", "ul", "table", "thead", "tbody", "tfoot", "tr", "td", "th", "caption", "colgroup", "col"],
    ALLOWED_ATTR: ["size", "alt", "href", "src", "style", "width", "height", "color", "face", "align", "valign", "cellpadding", "cellspacing", "border", "colspan", "rowspan"],
    ALLOW_DATA_ATTR: false, ALLOW_ARIA_ATTR: false,
  });
  container.querySelectorAll<HTMLElement>("*").forEach((element) => {
    const style = element.style;
    const safe = document.createElement("span").style;
    for (const property of Array.from(style)) {
      if (!SIGNATURE_CSS.has(property)) continue;
      const value = sanitizeCssDeclaration(property, style.getPropertyValue(property));
      if (value !== null) safe.setProperty(property, value);
    }
    element.removeAttribute("style");
    if (safe.cssText) element.setAttribute("style", safe.cssText);
    for (const attribute of ["width", "height", "cellspacing", "border", "colspan", "rowspan"]) {
      if (!element.hasAttribute(attribute)) continue;
      const value = sanitizeHtmlDimension(element.getAttribute(attribute)!, attribute === "width" || attribute === "height");
      if (value === null) element.removeAttribute(attribute); else element.setAttribute(attribute, value);
    }
    if (element.hasAttribute("cellpadding")) {
      const value = sanitizeHtmlCellPadding(element.getAttribute("cellpadding")!);
      if (value === null) element.removeAttribute("cellpadding"); else element.setAttribute("cellpadding", value);
    }
    for (const [attribute, values] of [["align", ["left", "center", "right", "justify"]], ["valign", ["top", "middle", "bottom", "baseline"]]] as const) {
      if (element.hasAttribute(attribute) && !values.includes(element.getAttribute(attribute)!.toLowerCase() as never)) element.removeAttribute(attribute);
    }
    if (element.hasAttribute("color") && !sanitizeCssDeclaration("color", element.getAttribute("color")!)) element.removeAttribute("color");
    if (element.hasAttribute("size")) {
      const size = SIGNATURE_FONT_SIZES[element.getAttribute("size")!];
      element.removeAttribute("size");
      if (element.tagName === "FONT" && size) element.style.fontSize = size;
    }
    if (element.hasAttribute("face")) {
      const family = sanitizeCssDeclaration("font-family", element.getAttribute("face")!);
      element.removeAttribute("face");
      if (family) element.style.fontFamily = family;
    }
  });
  container.querySelectorAll("a").forEach((link) => {
    if (!/^(https?:\/\/|mailto:|tel:)/i.test(link.getAttribute("href") ?? "")) link.removeAttribute("href");
    // Preview/editor links never navigate away from the app.
  });
  container.querySelectorAll("img").forEach((image) => {
    const source = image.getAttribute("src") ?? "";
    if (!/^data:image\/(?:avif|gif|jpeg|png|webp);base64,[a-z0-9+/]+=*$/i.test(source)
      && !(allowCid && /^cid:[a-z0-9._@-]+$/i.test(source))) image.remove();
  });
  return container.innerHTML;
}

export function readSignatureEditor(editor: HTMLElement): { html: string; text: string } {
  const clone = editor.cloneNode(true) as HTMLElement;
  clone.querySelectorAll<HTMLElement>("[data-compose-image]").forEach((wrapper) => {
    const image = wrapper.querySelector("img");
    if (image?.dataset.composeSource) image.setAttribute("src", image.dataset.composeSource);
    if (image) wrapper.replaceWith(image); else wrapper.remove();
  });
  const html = sanitizeSignatureHtml(clone.innerHTML, true);
  clone.innerHTML = html;
  // Table cells need separation, and logos should have an accessible text fallback.
  clone.querySelectorAll("img").forEach((image) => image.replaceWith(document.createTextNode(image.alt)));
  clone.querySelectorAll("a[href]").forEach((link) => {
    const href = link.getAttribute("href")!;
    const target = /^mailto:|^tel:/i.test(href) ? href.replace(/^(mailto:|tel:)/i, "").split("?")[0] : href;
    const label = link.textContent?.trim() ?? "";
    if (!label) link.textContent = target;
    else if (label !== target && label !== href) link.append(document.createTextNode(` (${target})`));
  });
  clone.querySelectorAll("td,th").forEach((cell) => cell.append(document.createTextNode(" ")));
  clone.querySelectorAll("tr").forEach((row) => row.append(document.createElement("br")));
  return { html, text: composeHtmlToText(clone).trim() };
}

export function validateSignature(signature: Signature): void {
  if (!signature.name.trim() || /[\r\n\0]/.test(signature.name) || Array.from(signature.name).length > SIGNATURE_LIMITS.maxNameLength) throw new Error("Enter a signature name of 200 characters or fewer.");
  if (!signature.html.trim()) throw new Error("Add text or an image to your signature.");
  if (new TextEncoder().encode(signature.html).length > SIGNATURE_LIMITS.maxHtmlBytes || new TextEncoder().encode(signature.text).length > SIGNATURE_LIMITS.maxTextBytes) throw new Error("This signature is too large. Use a smaller image or less text.");
}

export function draftSignatureSnapshot(signature: Signature | null, automatic = false): DraftSignature {
  return { ...(signature ?? { id: null, name: "No Signature", html: "", text: "" }), automatic, htmlOffset: 0, textOffset: 0 };
}

export function signatureBody(draft: Draft): { html: string; text: string } {
  const s = draft.signature;
  const insert = (value: string, offset: number, content: string, separator: string) => `${value.slice(0, offset)}${offset && !value.slice(0, offset).endsWith(separator) ? separator : ""}${content}${offset < value.length && !value.slice(offset).startsWith(separator) ? separator : ""}${value.slice(offset)}`;
  return s?.html ? { html: insert(draft.bodyHtml ?? "", s.htmlOffset, s.html, "<br><br>"), text: insert(draft.body, s.textOffset, s.text, "\n\n") } : { html: draft.bodyHtml ?? "", text: draft.body };
}
