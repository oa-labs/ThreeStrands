/** Inline formatting whose edge spaces the Markdown converter can't write as
 * plain spaces: `**Parking: **` doesn't close, so it writes `&#x20;`. */
const FORMATTING = "a, abbr, b, big, cite, del, dfn, em, font, i, ins, mark, q, s, small, span, strike, strong, sub, sup, time, u, var";
/** Content whose spacing is part of the text. */
const PRESERVED = "pre, code, kbd, samp, textarea, script, style";
/** Elements that are content even without text. */
const SUBSTANTIAL = "img, br, hr, input, svg, video, audio, iframe, object, embed, canvas, picture";
const NBSP = /\u00a0/g;
const HAS_NBSP = /\u00a0/;
const LEADING = /^[ \t\r\n\f]+/;
const TRAILING = /[ \t\r\n\f]+$/;
/** Flow containers whose boundaries must stay outside Markdown inline marks. */
const BLOCKS = "address, article, aside, blockquote, dd, details, div, dl, dt, fieldset, figcaption, figure, footer, form, h1, h2, h3, h4, h5, h6, header, hr, li, main, nav, ol, p, pre, section, summary, table, tbody, td, tfoot, th, thead, tr, ul";

/** Apply an inline mark to runs of inline content, never around whole blocks. */
function markInlineContent(element: Element, tag: string) {
  if (element.matches(PRESERVED)) return;
  const equivalent = tag === "strong" ? "b, strong" : tag === "em" ? "i, em" : "s, strike, del";
  let run: Element | undefined;
  for (const child of [...element.childNodes]) {
    if (child instanceof Element && child.matches(`${PRESERVED}, ${equivalent}`)) {
      run = undefined;
    } else if (child instanceof Element && (child.matches(BLOCKS) || child.querySelector(`${BLOCKS}, ${equivalent}`))) {
      run = undefined;
      markInlineContent(child, tag);
    } else {
      if (!run) { run = element.ownerDocument.createElement(tag); child.before(run); }
      run.append(child);
    }
  }
}

/**
 * Rich-text clipboards use CSS text runs (and sometimes a non-bold <b>
 * around the entire selection). The converter reads semantic tags, not CSS.
 * Translate only Markdown-supported emphasis; never import CSS capabilities.
 */
function normalizeFormatting(root: HTMLElement): boolean {
  let changed = false;
  for (const element of [...root.querySelectorAll<HTMLElement>("[style], b, strong, i, em, s, strike, del")].reverse()) {
    if (element.closest(PRESERVED)) continue;
    const weight = element.style.fontWeight;
    const bold = weight === "bold" || (Number(weight) >= 600);
    const normalWeight = weight === "normal" || (weight !== "" && Number(weight) < 600);
    const italic = element.style.fontStyle === "italic" || element.style.fontStyle === "oblique";
    const normalStyle = element.style.fontStyle === "normal";
    const strike = /\bline-through\b/.test(element.style.textDecorationLine || element.style.textDecoration);
    const tag = element.tagName.toLowerCase();
    const isBold = tag === "b" || tag === "strong";
    const isItalic = tag === "i" || tag === "em";
    const isStrike = ["s", "strike", "del"].includes(tag);
    // A mark spanning blocks makes the converter serialize the entire
    // selection as one inline run, losing the block separators.
    const spansBlocks = Boolean(element.querySelector(BLOCKS));
    const unwrap = (isBold && (normalWeight || spansBlocks)) || (isItalic && (normalStyle || spansBlocks)) || (isStrike && spansBlocks);
    if ((bold || (isBold && !normalWeight && spansBlocks)) && (!isBold || unwrap)) {
      markInlineContent(element, "strong"); changed = true;
    }
    if ((italic || (isItalic && !normalStyle && spansBlocks)) && (!isItalic || unwrap)) {
      markInlineContent(element, "em"); changed = true;
    }
    if ((strike || (isStrike && spansBlocks)) && (!isStrike || unwrap)) { markInlineContent(element, "s"); changed = true; }
    if (unwrap) { element.replaceWith(...element.childNodes); changed = true; }
  }
  return changed;
}

function textNodes(root: Node): Text[] {
  const walker = root.ownerDocument!.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const found: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) found.push(node as Text);
  return found;
}

/** Strips whitespace from one edge of `element`'s text; true when there was some. */
function stripEdge(element: Element, edge: "start" | "end"): boolean {
  const nodes = textNodes(element);
  if (edge === "end") nodes.reverse();
  const pattern = edge === "start" ? LEADING : TRAILING;
  let stripped = false;
  for (const node of nodes) {
    const value = node.data;
    const next = value.replace(pattern, "");
    if (next !== value) { node.data = next; stripped = true; }
    if (next) break;
  }
  return stripped;
}

/**
 * Pasted mail and web HTML, cleaned so the editor's Markdown conversion
 * doesn't write character references such as `&#x20;` and `&#xA0;` into the
 * note, which the editor would show as typed text. CSS emphasis becomes semantic
 * formatting, with block boundaries kept outside inline marks. Non-breaking
 * spaces become spaces outside code, spaces at the edges of formatting move outside
 * it, and formatting left with nothing in it is dropped. Null when nothing
 * changed, so the paste proceeds untouched.
 */
export function cleanPastedHtml(html: string): string | null {
  // The editor's own copies already hold the note's Markdown.
  if (html.includes("data-pm-slice") || html.includes("data-meowdown")) return null;
  const doc = new DOMParser().parseFromString(html, "text/html");
  let changed = normalizeFormatting(doc.body);
  for (const node of textNodes(doc.body)) {
    if (node.parentElement?.closest(PRESERVED) || !HAS_NBSP.test(node.data)) continue;
    node.data = node.data.replace(NBSP, " ");
    changed = true;
  }
  // Innermost first, so a space works its way out of nested formatting.
  for (const element of [...doc.body.querySelectorAll(FORMATTING)].reverse()) {
    if (element.closest(PRESERVED)) continue;
    if (stripEdge(element, "start")) { element.before(" "); changed = true; }
    if (stripEdge(element, "end")) { element.after(" "); changed = true; }
    if (!element.textContent && !element.querySelector(SUBSTANTIAL)) { element.remove(); changed = true; }
  }
  return changed ? doc.body.innerHTML : null;
}
