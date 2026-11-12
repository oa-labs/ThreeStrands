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
 * note, which the editor would show as typed text. Non-breaking spaces become
 * spaces outside code, spaces at the edges of inline formatting move outside
 * it, and formatting left with nothing in it is dropped. Null when nothing
 * changed, so the paste proceeds untouched.
 */
export function cleanPastedHtml(html: string): string | null {
  // The editor's own copies already hold the note's Markdown.
  if (html.includes("data-pm-slice") || html.includes("data-meowdown")) return null;
  const doc = new DOMParser().parseFromString(html, "text/html");
  let changed = false;
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
