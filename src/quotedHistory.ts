/**
 * Quoted-history folding: finds where a reply's current content ends and the
 * quoted thread begins, using provider-neutral evidence only.
 *
 * Detection works on logical lines rather than individual DOM text nodes.
 * Sender markup and ThreeStrands' own linkification routinely split one
 * visible line ("On …, A. Sender <a@b.example> wrote:") across several text
 * nodes and inline elements, so the HTML is first flattened into lines the way
 * a reader sees them — block elements and <br> break lines, inline elements do
 * not — while every line keeps a map back to its DOM position for the cut.
 *
 * This is visual normalization, not a security decision: callers pass HTML
 * the security stages already sanitized, and the full message stays available
 * when the reader expands the folded part.
 */
import { EMAIL_QUOTE_FOLDING_LIMITS as LIMITS } from "./emailRenderingPolicy";

export type QuotedHistoryBoundary =
  | { node: Element; kind: "element" }
  | { node: Text; kind: "text"; offset: number };

/** Evidence weights; see docs/email-rendering-policy.md. */
const SCORE = {
  separatorMarker: 3,
  completeHeaderCluster: 3,
  headerCluster: 2,
  precedingRule: 1,
  quotedRegion: 2,
  quotedLineRun: 3,
  pairedEvidence: 2,
  currentContent: 1,
} as const;

const quotedHistoryMarker = /(?:^|\n)\s*(?:(?:[-—_]{2,})\s*)?(?:original message|forwarded message|begin forwarded message):?(?:\s*(?:[-—_]{2,}))?\s*(?:\n|$)/i;
const wroteMarker = new RegExp(String.raw`(?:^|\n)\s*On\s+[^\n]{1,${LIMITS.maxAttributionLength}}\s+wrote:\s*(?:\n|$)`, "i");
const headerField = /^(from|sent|date|to|cc|bcc|subject)\s*:/i;
const emailOrTimestamp = /(?:[\w.+-]+@[\w.-]+\.[a-z]{2,}|\b\d{1,2}:\d{2}\b|\b\d{4}[-/]\d{1,2}[-/]\d{1,2}\b)/i;
const separatorLine = /^\s*[-—_=]{2,}\s*$/;
const quoteLine = /^\s*>/;

/**
 * Mail clients sometimes hard-wrap the "On <date>, <name> <email> wrote:"
 * line — most often when a long display name or address pushes "wrote:"
 * past the wrap column — landing "wrote:" (optionally with the address) on
 * its own line. These patterns recognize an opener ending mid-phrase and that
 * continuation so the pair can still be treated as one boundary, regardless
 * of which client produced the wrap.
 */
const wroteOpenerLine = new RegExp(String.raw`(?:^|\n)\s*On\s+\S[^\n]{0,${LIMITS.maxAttributionLength - 1}}$`, "i");
const wroteContinuationLine = /^\s*(?:<[^<>\s]+@[^<>\s]+>\s*)?wrote:\s*$/i;

/** Elements that start a new visible line; everything else flows inline. */
const LINE_BREAKING_ELEMENTS = new Set([
  "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "CAPTION", "CENTER", "DD", "DIV", "DL", "DT",
  "FIGCAPTION", "FIGURE", "FOOTER", "H1", "H2", "H3", "H4", "H5", "H6", "HEADER", "HR", "LI",
  "MAIN", "NAV", "OL", "P", "PRE", "SECTION", "TABLE", "TBODY", "TD", "TFOOT", "TH", "THEAD",
  "TR", "UL",
]);

type FlatText = {
  text: string;
  segments: Array<{ node: Text; start: number }>;
  spans: Map<Element, { start: number; end: number }>;
};

type Line = { text: string; start: number; end: number; blank: boolean };

type Evidence = { lineStart: number; offset: number; base: number; boundary: QuotedHistoryBoundary | null };

type Region = { start: number; end: number; base: number; boundary: QuotedHistoryBoundary | null };

/**
 * Flattens the DOM into reader-visible lines. Source-formatting whitespace
 * (newlines and indentation between blocks) is dropped or turned into spaces
 * without changing offsets inside a text node, so every flat offset still
 * maps to a DOM position. Preformatted text keeps its own line breaks.
 */
function flatten(container: Element): FlatText {
  let text = "";
  const segments: FlatText["segments"] = [];
  const spans: FlatText["spans"] = new Map();
  const breakLine = () => {
    if (text && !text.endsWith("\n")) text += "\n";
  };
  const visit = (node: Node, preformatted: boolean) => {
    if (node.nodeType === Node.TEXT_NODE) {
      const data = (node as Text).data;
      if (!preformatted && /^[ \t\r\n]*$/.test(data) && (!text || text.endsWith("\n"))) return;
      segments.push({ node: node as Text, start: text.length });
      text += preformatted ? data.replace(/\r/g, " ") : data.replace(/[\r\n\t]/g, " ");
      return;
    }
    if (!(node instanceof Element)) return;
    if (node.tagName === "BR") {
      text += "\n";
      return;
    }
    const breaks = LINE_BREAKING_ELEMENTS.has(node.tagName);
    if (breaks) breakLine();
    const start = text.length;
    const pre = preformatted || node.tagName === "PRE"
      || /^pre/i.test((node as HTMLElement).style?.whiteSpace ?? "");
    node.childNodes.forEach((child) => visit(child, pre));
    spans.set(node, { start, end: text.length });
    if (breaks) breakLine();
  };
  container.childNodes.forEach((child) => visit(child, false));
  return { text, segments, spans };
}

function splitLines(text: string): Line[] {
  const lines: Line[] = [];
  let start = 0;
  for (const part of text.split("\n")) {
    lines.push({ text: part, start, end: start + part.length, blank: !part.trim() });
    start += part.length + 1;
  }
  return lines;
}

function containsImage(node: Node): boolean {
  return node instanceof Element && (node.tagName === "IMG" || node.querySelector("img") !== null);
}

/**
 * The DOM position where the line at `offset` begins. When that line starts
 * an element (or a chain of them) the cut moves before the outermost such
 * element, so the visible copy does not end in empty clones of the quoted
 * wrappers — for example an empty blockquote border.
 */
function boundaryAt(flat: FlatText, container: Element, offset: number): QuotedHistoryBoundary | null {
  const segment = flat.segments.find(({ node, start }) => start + node.data.length > offset);
  if (!segment) return null;
  const textOffset = Math.max(0, offset - segment.start);
  if (textOffset > 0) return { kind: "text", node: segment.node, offset: textOffset };

  let node: Node = segment.node;
  let hoisted: Element | null = null;
  while (node.parentElement && node.parentElement !== container) {
    const parent: Element = node.parentElement;
    if (flat.spans.get(parent)?.start !== segment.start) break;
    let sibling = node.previousSibling;
    while (sibling && !containsImage(sibling)) sibling = sibling.previousSibling;
    if (sibling) break;
    hoisted = parent;
    node = parent;
  }
  return hoisted ? { kind: "element", node: hoisted } : { kind: "text", node: segment.node, offset: 0 };
}

function isAttributionLine(lines: Line[], index: number): boolean {
  const line = lines[index].text;
  if (quotedHistoryMarker.test(line) || wroteMarker.test(line)) return true;
  if (!wroteOpenerLine.test(line) || /wrote:/i.test(line)) return false;
  let joinedLength = line.trim().length;
  let seen = 0;
  for (let next = index + 1; next < lines.length && seen < LIMITS.wrappedAttributionLookaheadLines; next++) {
    if (lines[next].blank) return false;
    seen++;
    joinedLength += 1 + lines[next].text.trim().length;
    if (wroteContinuationLine.test(lines[next].text)) return joinedLength <= LIMITS.maxAttributionLength;
  }
  return false;
}

/** Base score for a From/Sent/To/Subject cluster starting at `index`, or 0. */
function headerClusterScore(lines: Line[], index: number): number {
  if (lines[index].blank || !headerField.test(lines[index].text.trim())) return 0;
  const block: string[] = [];
  for (let next = index; next < lines.length && !lines[next].blank && block.length < LIMITS.maxHeaderClusterLines; next++) {
    block.push(lines[next].text.trim());
  }
  const fields = new Set(block.map((line) => line.match(headerField)?.[1].toLowerCase()).filter(Boolean));
  if (!fields.has("from") || fields.size < 3 || !emailOrTimestamp.test(block.join(" "))) return 0;
  const complete = fields.size >= 4 && (fields.has("sent") || fields.has("date"));
  return complete ? SCORE.completeHeaderCluster : SCORE.headerCluster;
}

function hasMeaningfulFollowingContent(element: Element, container: Element): boolean {
  let current: Element = element;
  while (current.parentElement && current.parentElement !== container) {
    if (Array.from(current.parentElement.children).slice(Array.from(current.parentElement.children).indexOf(current) + 1)
      .some((sibling) => Boolean(sibling.textContent?.trim()) || sibling.querySelector("img"))) return true;
    current = current.parentElement;
  }
  if (current.parentElement === container) {
    return Array.from(container.children).slice(Array.from(container.children).indexOf(current) + 1)
      .some((sibling) => Boolean(sibling.textContent?.trim()) || sibling.querySelector("img"));
  }
  return false;
}

/**
 * Start line of a trailing `>`-quoted region: at least `minQuoteRunLines`
 * quoted lines with nothing but blank lines between them and the end of the
 * message. Quoted lines followed by unquoted text (an inline reply) never
 * count, so the reader's interleaved answers stay visible.
 */
function trailingQuoteRunStart(lines: Line[]): number {
  let start = -1;
  let quoted = 0;
  for (let index = lines.length - 1; index >= 0; index--) {
    if (lines[index].blank) continue;
    if (!quoteLine.test(lines[index].text)) break;
    start = index;
    quoted++;
  }
  return quoted >= LIMITS.minQuoteRunLines ? start : -1;
}

/**
 * Text already seen earlier in the thread, as 4-word shingles. Repeated text
 * is fill-in evidence only: it extends a structural fold upward over a
 * repeated signature, confirms a lone citation, or — with a larger minimum —
 * folds a trailing run that carries no structural markers at all.
 */
export type PriorThreadText = { has(shingle: string): boolean };

/** Every earlier message's shingles, so position k sees only messages before k. */
export type ThreadTextIndex = { before(position: number): PriorThreadText };

const quoteWord = /[\p{L}\p{N}]+/gu;

/** Normalized words of one line: quote markers, case and punctuation are ignored. */
function lineWords(line: string): string[] {
  return line.replace(/^(?:\s*>)+/, "").toLowerCase().match(quoteWord) ?? [];
}

function shingleAt(words: readonly string[], start: number): string {
  return words.slice(start, start + LIMITS.shingleWords).join(" ");
}

/** Indexes each message's text in thread order; texts are plain text, oldest first. */
export function buildThreadTextIndex(texts: readonly string[]): ThreadTextIndex {
  const firstSeen = new Map<string, number>();
  texts.forEach((text, position) => {
    const words = text.split(/\r?\n/).flatMap(lineWords);
    for (let start = 0; start + LIMITS.shingleWords <= words.length; start++) {
      const shingle = shingleAt(words, start);
      if (!firstSeen.has(shingle)) firstSeen.set(shingle, position);
    }
  });
  return {
    before: (position) => ({ has: (shingle) => (firstSeen.get(shingle) ?? Infinity) < position }),
  };
}

type RepeatedLine = { kind: "neutral" | "seen" | "new"; matched: number };

/**
 * Classifies each line against earlier thread text. Shingles run across line
 * breaks, so a short line ("Joel") or a rewrapped quote still matches. A line
 * with no words is neutral; otherwise it is seen when matched shingles cover
 * at least `minSeenLineCoverage` of its words.
 */
function classifyRepeatedLines(lines: readonly string[], prior: PriorThreadText): RepeatedLine[] {
  const words: Array<{ word: string; line: number }> = [];
  lines.forEach((line, index) => lineWords(line).forEach((word) => words.push({ word, line: index })));
  const plain = words.map(({ word }) => word);
  const covered = new Array<boolean>(words.length).fill(false);
  const matched = new Array<number>(lines.length).fill(0);
  for (let start = 0; start + LIMITS.shingleWords <= words.length; start++) {
    if (!prior.has(shingleAt(plain, start))) continue;
    matched[words[start].line]++;
    covered.fill(true, start, start + LIMITS.shingleWords);
  }
  const totals = new Array<number>(lines.length).fill(0);
  const coveredCounts = new Array<number>(lines.length).fill(0);
  words.forEach(({ line }, index) => {
    totals[line]++;
    if (covered[index]) coveredCounts[line]++;
  });
  return lines.map((_, index) => ({
    kind: totals[index] === 0 ? "neutral"
      : coveredCounts[index] / totals[index] >= LIMITS.minSeenLineCoverage ? "seen" : "new",
    matched: matched[index],
  }));
}

/** The contiguous run of seen or wordless lines ending just above `end`. */
function repeatedRunAbove(lines: readonly RepeatedLine[], end: number): { top: number; matched: number } {
  let top = end;
  let matched = 0;
  for (let index = end - 1; index >= 0 && lines[index].kind !== "new"; index--) {
    top = index;
    matched += lines[index].matched;
  }
  return { top, matched };
}

/** Marks where the folded part begins so the reader can position the fold toggle. */
export const QUOTED_HISTORY_FOLD_ATTRIBUTE = "data-quoted-history-fold";

type Fold = { boundary: QuotedHistoryBoundary; visible: string };

/**
 * The two renderings of a reply with quoted history: `visible` is the HTML
 * before the quoted section and `expanded` is the full HTML. Both end the
 * visible part with the same fold marker, so the toggle the reader clicks
 * sits at the same place whether the quoted section is shown or hidden.
 */
export type QuotedHistoryFold = { visible: string; expanded: string };

/**
 * Returns the message HTML before a mail client's quoted-reply section.
 * The full sanitized HTML remains available to reveal after the reader asks
 * for it; only this shorter copy is placed in the iframe initially.
 */
export function collapseQuotedHistoryHtml(html: string, prior?: PriorThreadText): string | null {
  const container = document.createElement("div");
  container.innerHTML = html;
  return findQuotedHistoryFold(container, prior)?.visible ?? null;
}

/** Like collapseQuotedHistoryHtml, plus the full HTML with a fold marker at the boundary. */
export function foldQuotedHistoryHtml(html: string, prior?: PriorThreadText): QuotedHistoryFold | null {
  const container = document.createElement("div");
  container.innerHTML = html;
  container.querySelectorAll(`[${QUOTED_HISTORY_FOLD_ATTRIBUTE}]`).forEach((node) => node.removeAttribute(QUOTED_HISTORY_FOLD_ATTRIBUTE));
  const fold = findQuotedHistoryFold(container, prior);
  if (!fold) return null;

  const createMarker = () => {
    const marker = document.createElement("span");
    marker.setAttribute(QUOTED_HISTORY_FOLD_ATTRIBUTE, "");
    marker.setAttribute("aria-hidden", "true");
    return marker;
  };
  const visible = document.createElement("div");
  visible.innerHTML = fold.visible;
  visible.append(createMarker());

  const { boundary } = fold;
  const marker = createMarker();
  if (boundary.kind === "element") boundary.node.before(marker);
  else if (boundary.offset > 0) boundary.node.splitText(boundary.offset).before(marker);
  else boundary.node.before(marker);
  return { visible: visible.innerHTML, expanded: container.innerHTML };
}

function findQuotedHistoryFold(container: Element, prior?: PriorThreadText): Fold | null {
  const flat = flatten(container);
  const lines = splitLines(flat.text);
  const rules = Array.from(container.querySelectorAll("hr"))
    .map((rule) => ({ rule, start: flat.spans.get(rule)?.start ?? -1 }));

  const attributions: Evidence[] = [];
  for (let index = 0; index < lines.length; index++) {
    if (lines[index].blank) continue;
    const lineStart = lines[index].start;
    if (isAttributionLine(lines, index)) {
      attributions.push({ lineStart, offset: lineStart, base: SCORE.separatorMarker, boundary: boundaryAt(flat, container, lineStart) });
      continue;
    }
    const headerScore = headerClusterScore(lines, index);
    if (!headerScore) continue;
    let previous = index - 1;
    while (previous >= 0 && lines[previous].blank) previous--;
    const afterPrevious = previous >= 0 ? lines[previous].end : -1;
    const rule = rules.find(({ start }) => start > afterPrevious && start <= lineStart);
    const separator = previous >= 0 && separatorLine.test(lines[previous].text) ? lines[previous] : null;
    const evidence: Evidence = { lineStart, offset: lineStart, base: headerScore, boundary: boundaryAt(flat, container, lineStart) };
    if (separator) {
      Object.assign(evidence, { offset: separator.start, base: headerScore + SCORE.precedingRule, boundary: boundaryAt(flat, container, separator.start) });
    } else if (rule) {
      Object.assign(evidence, { offset: rule.start, base: headerScore + SCORE.precedingRule, boundary: { kind: "element", node: rule.rule } });
    }
    attributions.push(evidence);
    while (index + 1 < lines.length && !lines[index + 1].blank) index++;
  }

  const regions: Region[] = Array.from(container.querySelectorAll("blockquote, cite"))
    .filter((node) => !hasMeaningfulFollowingContent(node, container))
    .map((node) => ({ ...(flat.spans.get(node) ?? { start: 0, end: 0 }), base: SCORE.quotedRegion, boundary: { kind: "element", node } }));
  const quoteRun = trailingQuoteRunStart(lines);
  if (quoteRun >= 0) {
    regions.push({ start: lines[quoteRun].start, end: flat.text.length, base: SCORE.quotedLineRun, boundary: boundaryAt(flat, container, lines[quoteRun].start) });
  }

  const opensRegion = (attribution: Evidence, region: Region) => region.start <= attribution.lineStart
    && attribution.lineStart < region.end
    && !lines.some((line) => !line.blank && line.start >= region.start && line.start < attribution.lineStart);

  const candidates = [
    ...attributions.map((attribution) => ({
      offset: attribution.offset,
      boundary: attribution.boundary,
      score: attribution.base + (regions.some((region) => region.start >= attribution.lineStart || opensRegion(attribution, region))
        ? SCORE.pairedEvidence : 0),
    })),
    ...regions.map((region) => ({
      offset: region.start,
      boundary: region.boundary,
      score: region.base + (attributions.some((attribution) => opensRegion(attribution, region)) ? SCORE.pairedEvidence : 0),
    })),
  ].sort((left, right) => left.offset - right.offset);

  const visibleBefore = (boundary: QuotedHistoryBoundary | null): Fold | null => {
    if (!boundary) return null;
    const range = document.createRange();
    range.setStart(container, 0);
    if (boundary.kind === "text") range.setEnd(boundary.node, boundary.offset);
    else range.setEndBefore(boundary.node);

    const visibleContainer = document.createElement("div");
    visibleContainer.append(range.cloneContents());
    const hasVisibleContent = Boolean(visibleContainer.textContent?.trim())
      || visibleContainer.querySelector("img") !== null;
    return hasVisibleContent ? { boundary, visible: visibleContainer.innerHTML } : null;
  };
  const lineAt = (offset: number) => {
    let index = 0;
    while (index + 1 < lines.length && lines[index + 1].start <= offset) index++;
    return index;
  };

  let structural: { line: number; fold: Fold } | null = null;
  for (const candidate of candidates) {
    if (candidate.score + SCORE.currentContent < LIMITS.foldScoreThreshold) continue;
    const fold = visibleBefore(candidate.boundary);
    if (fold !== null) {
      structural = { line: lineAt(candidate.offset), fold };
      break;
    }
  }
  if (!prior) return structural?.fold ?? null;

  const repeated = classifyRepeatedLines(lines.map((line) => line.text), prior);
  const foldAtLine = (line: number) => visibleBefore(boundaryAt(flat, container, lines[line].start));
  if (structural) {
    const run = repeatedRunAbove(repeated, structural.line);
    if (run.top < structural.line && run.matched >= LIMITS.minCorroboratingShingles) {
      return foldAtLine(run.top) ?? structural.fold;
    }
    return structural.fold;
  }
  const run = repeatedRunAbove(repeated, lines.length);
  if (run.top >= lines.length) return null;
  const confirmsCitation = regions.some((region) => region.start >= lines[run.top].start);
  const needed = confirmsCitation ? LIMITS.minCorroboratingShingles : LIMITS.minRepeatedRegionShingles;
  return run.matched >= needed ? foldAtLine(run.top) : null;
}

/** Index of the first line starting a run of minQuoteRunLines+ consecutive `>`-quoted lines, or -1. */
function findQuoteRunStart(lines: string[]): number {
  let runStart = -1;
  let runLength = 0;
  for (let index = 0; index < lines.length; index++) {
    if (quoteLine.test(lines[index])) {
      if (runLength === 0) runStart = index;
      runLength++;
      if (runLength >= LIMITS.minQuoteRunLines) return runStart;
    } else {
      runLength = 0;
      runStart = -1;
    }
  }
  return -1;
}

/** Line index where a plain-text reply's quoted history starts, from structure alone, or -1. */
function structuralTextCut(lines: string[]): number {
  const plainLines = lines.map((line) => ({ text: line, start: 0, end: 0, blank: !line.trim() }));
  const markerIndex = lines.findIndex((_, index) => isAttributionLine(plainLines, index));
  const quoteRunIndex = findQuoteRunStart(lines);
  const cutCandidates = [markerIndex, quoteRunIndex].filter((index) => index >= 0);
  if (cutCandidates.length > 0) return Math.min(...cutCandidates);
  const headerStart = lines.findIndex((_, index) => {
    const block = lines.slice(index, index + LIMITS.maxHeaderClusterLines).map((line) => line.trim()).filter(Boolean);
    const fields = new Set(block.filter((line) => headerField.test(line)).map((line) => line.match(headerField)?.[1].toLowerCase()));
    return fields.size >= 3 && emailOrTimestamp.test(block.join(" "));
  });
  if (headerStart > 0 && lines.slice(0, headerStart).some((line) => line.trim())) {
    const separator = lines.slice(0, headerStart).some((line) => /^\s*[-—_]{2,}\s*$/.test(line));
    if (separator) return headerStart;
  }
  return -1;
}

/** Returns the part of a plain-text reply before its quoted history. */
export function collapseQuotedHistoryText(text: string, prior?: PriorThreadText): string | null {
  return foldQuotedHistoryText(text, prior)?.visible ?? null;
}

/** Splits a plain-text reply into the part before its quoted history and the quoted part. */
export function foldQuotedHistoryText(text: string, prior?: PriorThreadText): { visible: string; quoted: string } | null {
  const lines = text.split(/\r?\n/);
  const visibleBefore = (cut: number) => {
    const visible = lines.slice(0, cut).join("\n").trimEnd();
    return visible.trim() ? { visible, quoted: lines.slice(cut).join("\n").replace(/^\n+/, "") } : null;
  };
  const cut = structuralTextCut(lines);
  if (!prior) return cut >= 0 ? visibleBefore(cut) : null;

  const repeated = classifyRepeatedLines(lines, prior);
  if (cut >= 0) {
    const run = repeatedRunAbove(repeated, cut);
    if (run.top < cut && run.matched >= LIMITS.minCorroboratingShingles) {
      return visibleBefore(run.top) ?? visibleBefore(cut);
    }
    return visibleBefore(cut);
  }
  const run = repeatedRunAbove(repeated, lines.length);
  return run.top < lines.length && run.matched >= LIMITS.minRepeatedRegionShingles ? visibleBefore(run.top) : null;
}
