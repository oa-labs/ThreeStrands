/**
 * `[[wiki links]]` by the native rules (src-tauri/src/notes.rs), for the
 * browser demo and the editor: `[[Title|shown text]]` links to Title, and
 * links in fenced code, inline code, or after a backslash don't count.
 */

/**
 * Whitespace as Rust's `char::is_whitespace` has it (Unicode White_Space),
 * which `split_whitespace` splits on natively. JavaScript's `\s` differs: it
 * counts U+FEFF and not U+0085.
 */
const WHITESPACE = /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+/;

/** `text` with runs of whitespace collapsed to one space and the ends trimmed, as natively. */
export const collapseWhitespace = (text: string) => text.split(WHITESPACE).filter(Boolean).join(" ");

/**
 * The key a title is matched by: whitespace collapsed and case folded, as
 * `title_key` natively. Folding ignores the system's language, as Rust's
 * `to_lowercase` does, so "I" folds to "i" under a Turkish locale too.
 */
export const noteTitleKey = (title: string) => collapseWhitespace(title).toLowerCase();

/** A `[[...]]` link's target and the text it shows. */
export function splitWikiLink(inner: string): { target: string; label: string } {
  const bar = inner.indexOf("|");
  const target = collapseWhitespace(bar < 0 ? inner : inner.slice(0, bar));
  const label = bar < 0 ? target : inner.slice(bar + 1).trim() || target;
  return { target, label };
}

type Span = { title: string; start: number; end: number };

const fenceMarker = (line: string): [string, number] | null => {
  const match = /^ {0,3}(`{3,}|~{3,})/.exec(line);
  return match ? [match[1]![0]!, match[1]!.length] : null;
};

/**
 * The index just past the backtick run of exactly `run` that closes an
 * inline code span, as natively (`closing_backticks`): a longer or shorter
 * run is part of the code, not its end.
 */
function closingBackticks(line: string, from: number, run: number): number | null {
  let index = from;
  while (index < line.length) {
    if (line[index] !== "`") { index++; continue; }
    let length = 0;
    while (line[index + length] === "`") length++;
    if (length === run) return index + length;
    index += length;
  }
  return null;
}

/** The target spans of each link in one prose line, as code-unit offsets. */
function lineSpans(line: string): Span[] {
  const spans: Span[] = [];
  let index = 0;
  while (index < line.length) {
    if (line[index] === "`") {
      let run = 0;
      while (line[index + run] === "`") run++;
      const close = closingBackticks(line, index + run, run);
      index = close ?? index + run;
      continue;
    }
    if (line.startsWith("[[", index) && line[index - 1] !== "\\") {
      const close = line.indexOf("]]", index + 2);
      const inner = close < 0 ? "" : line.slice(index + 2, close);
      if (close >= 0 && !/[[\]]/.test(inner)) {
        const bar = inner.indexOf("|");
        const title = splitWikiLink(inner).target;
        if (title && [...title].length <= 200) {
          spans.push({ title, start: index + 2, end: index + 2 + (bar < 0 ? inner.length : bar) });
          index = close + 2;
          continue;
        }
      }
    }
    index++;
  }
  return spans;
}

/** Each line of `body` with its ending, and whether it is prose. */
function* lines(body: string): Generator<{ text: string; ending: string; prose: boolean }> {
  let fence: [string, number] | null = null;
  for (const piece of body.split(/(?<=\n)/)) {
    const text = piece.replace(/\r?\n$/, "");
    const ending = piece.slice(text.length);
    const marker = fenceMarker(text);
    if (marker && (!fence || (marker[0] === fence[0] && marker[1] >= fence[1]))) {
      fence = fence ? null : marker;
      yield { text, ending, prose: false };
      continue;
    }
    yield { text, ending, prose: !fence };
  }
}

/** Every link in `body`, in order, with the line it sits on. */
export function extractWikiLinks(body: string): { title: string; context: string }[] {
  const links: { title: string; context: string }[] = [];
  for (const line of lines(body)) {
    if (line.prose) for (const span of lineSpans(line.text)) links.push({ title: span.title, context: line.text.trim() });
  }
  return links;
}

/** `body` with links to the title keyed `oldKey` pointed at `newTitle`, or null when none changes. */
export function rewriteWikiLinks(body: string, oldKey: string, newTitle: string): string | null {
  let changed = false;
  let out = "";
  for (const line of lines(body)) {
    let text = line.text;
    if (line.prose) {
      for (const span of lineSpans(line.text).reverse()) {
        if (noteTitleKey(span.title) !== oldKey) continue;
        text = text.slice(0, span.start) + newTitle + text.slice(span.end);
        changed = true;
      }
    }
    out += text + line.ending;
  }
  return changed ? out : null;
}
