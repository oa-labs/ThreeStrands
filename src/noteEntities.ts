import { notePreview, noteInlineText } from "./notePreview";
import type { Note, NoteAttachment, NoteBacklink, ThreadTask } from "./domain";

/** An inert link from a note to an item elsewhere in the app. A contact
 * reference's `id` is the person's address, normalized as contact lookup
 * normalizes it; `contact` is the saved contact's id when there was one. */
export type NoteReference = { version: 1; kind: "conversation" | "task" | "event" | "contact"; id: string; account?: string; calendar?: string; note?: string; contact?: string;
  /** An event occurrence's recurring series, so the link also counts toward that series. */
  series?: string };
export type NoteItem = { id: string; title: string; checked: boolean; task: boolean; start: number; markerStart: number; markerEnd: number; end: number; persisted: boolean };
export type EntityLink = { reference: NoteReference; href: string; label: string; context: string; start: number };
export type NoteDocument = { items: NoteItem[]; links: EntityLink[]; warning: string | null };
export type NoteTaskRecord = { id: string; markerId: string; noteId: string; noteTitle: string; title: string; checked: boolean; updatedAt: string; completedAt: string | null; conflicted: boolean };
/** Where Add to Note writes: a note that exists (or a daily note that may not yet), or a new note. */
export type AddToNoteTarget = { kind: "note"; id: string } | { kind: "new"; title: string };
export type AddToNoteRequest = {
  target: AddToNoteTarget;
  /** The note's `updatedAt` the action began from; null for a missing daily note and a new note. */
  expected: string | null;
  thread: string;
  message: string;
  /** The passage to quote; null adds a link to the conversation. */
  quote: string | null;
  dateLabel: string;
  /** Attachment ids of the message to copy into the note. */
  files: string[];
};
export type NoteAddition = { note: Note; added: boolean; previousBody: string; createdNote: boolean; attachments: NoteAttachment[] };
export type UndoAddToNote = { noteId: string; expected: string; previousBody: string; createdNote: boolean; attachments: string[] };
export type { NoteBacklink };
export const MAX_NOTE_REFERENCE_BYTES = 2048;
export const MAX_NOTE_TASKS = 500;
export const MAX_NOTE_ENTITY_LINKS = 500;
const utf8 = (text: string) => new TextEncoder().encode(text).length;
const FIELDS = ["account", "calendar", "note", "contact", "series"] as const;

/** An address as contact lookup stores it: trimmed and ASCII-lowercased. */
export function normalizeContactAddress(address: string): string {
  return address.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
}
const contactAddress = (id: string) => /^[^\s@]+@[^\s@]+$/u.test(id) && normalizeContactAddress(id) === id;

export function parseNoteReference(href: string): NoteReference | null {
  if (utf8(href) > MAX_NOTE_REFERENCE_BYTES) return null;
  const match = /^threestrands:(conversation|task|event|contact)\/(.+)$/.exec(href);
  if (!match) return null;
  try {
    const r: unknown = JSON.parse(decodeURIComponent(match[2]));
    if (!r || typeof r !== "object" || Array.isArray(r)) return null;
    const v = r as Record<string, unknown>;
    if (Object.keys(v).some((k) => !["version", "kind", "id", ...FIELDS].includes(k))) return null;
    const valid = (v: unknown) => typeof v === "string" && v.length > 0 && !/[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(v);
    if (v.version !== 1 || v.kind !== match[1] || !valid(v.id)) return null;
    for (const k of FIELDS) if (k in v && !valid(v[k])) return null;
    if (v.kind !== "contact" && "contact" in v) return null;
    if ("series" in v && (v.kind !== "event" || v.series === v.id)) return null;
    if (v.kind === "conversation" && (!valid(v.account) || "calendar" in v || "note" in v)) return null;
    if (v.kind === "task" && ("account" in v || "calendar" in v)) return null;
    if (v.kind === "event" && (!valid(v.account) || !valid(v.calendar) || "note" in v)) return null;
    if (v.kind === "contact" && (!contactAddress(v.id as string) || "account" in v || "calendar" in v || "note" in v)) return null;
    return v as NoteReference;
  } catch { return null; }
}
export function noteReferenceHref(reference: NoteReference): string {
  const canonical = { version: reference.version, kind: reference.kind, id: reference.id, ...(reference.account !== undefined ? { account: reference.account } : {}), ...(reference.calendar !== undefined ? { calendar: reference.calendar } : {}), ...(reference.note !== undefined ? { note: reference.note } : {}), ...(reference.contact !== undefined ? { contact: reference.contact } : {}), ...(reference.series !== undefined ? { series: reference.series } : {}) };
  const href = `threestrands:${reference.kind}/${encodeURIComponent(JSON.stringify(canonical)).replace(/[!'()*._~-]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`)}`;
  if (!parseNoteReference(href)) throw new Error("Invalid note reference");
  return href;
}
/** The target a reference names. A contact reference names its address, so
 * links written with and without a contact id, or before and after a merge,
 * are one target. */
export function noteReferenceKey(r: NoteReference): string { return JSON.stringify([r.kind, r.account ?? null, r.calendar ?? null, r.note ?? null, r.id]); }
/** The series an event occurrence counts toward, keyed as a link to that whole series is. */
export function noteReferenceSeriesKey(r: NoteReference): string | null { return r.kind === "event" && r.series ? noteReferenceKey({ ...r, id: r.series, series: undefined }) : null; }
/** Contact references naming each of a contact's addresses, for its Notes
 * backlinks. Addresses a reference can't hold are left out. */
export function contactNoteReferences(addresses: string[]): NoteReference[] {
  return [...new Set(addresses.map(normalizeContactAddress))].flatMap((id) => {
    const reference: NoteReference = { version: 1, kind: "contact", id };
    try { noteReferenceHref(reference); return [reference]; } catch { return []; }
  });
}
const isWordChar = (ch: string | undefined) => ch !== undefined && /[\p{Alphabetic}\p{N}]/u.test(ch);
/** Escapes only what would change how the note editor reads a link label or
 * plain text, and turns line breaks into spaces. The editor shows Markdown
 * source, so every backslash is visible; inert punctuation such as `.`, `-`,
 * and `(` stays as written. `_` is escaped only at a word edge and `&` only
 * where it would begin an entity. Mirrors `escape_inline` in
 * `src-tauri/src/note_meetings.rs`. */
export function escapeMarkdownText(text: string): string {
  const chars = [...text.replace(/[\r\n]/g, " ")];
  return chars.map((ch, index) => {
    const escape = /[\\`*[\]<~=$#]/.test(ch)
      || (ch === "_" && !(isWordChar(chars[index - 1]) && isWordChar(chars[index + 1])))
      || (ch === "&" && /^(?:[A-Za-z0-9]{1,32}|#[0-9]{1,31}|#[xX][0-9A-Fa-f]{1,30});/.test(chars.slice(index + 1, index + 34).join("")));
    return escape ? `\\${ch}` : ch;
  }).join("");
}
export function markdownEntityLink(label: string, reference: NoteReference): string {
  return `[${escapeMarkdownText(label)}](${noteReferenceHref(reference)})`;
}
export function noteTaskAsTask(task: NoteTaskRecord): ThreadTask {
  return { id: task.id, accountId: "", threadId: null, subjectSnapshot: null, title: task.title, kind: "action", dueKind: "none", status: task.checked ? "completed" : "open", createdAt: "", updatedAt: task.updatedAt, completedAt: task.completedAt, source: "note", noteSource: task };
}

function indentationColumns(line: string): number {
  return [...(/^[ \t]*/.exec(line)?.[0] ?? "")].reduce((column, ch) => ch === "\t" ? column + 4 - column % 4 : column + 1, 0);
}

/** Source-range scanner for the editor adapter. Never rewrites a whole body
 * with a regex: fenced/indented code, quotes and comments remain literal.
 * Native owns durable ids and projection; shared fixtures keep the grammar aligned. */
export function noteItemRanges(body: string) { return scanListItems(body).items; }
function scanListItems(body: string) {
  /** Every list item line the scanner treats as live, task or not. */
  const lines: { start: number; end: number; prefix: string; bullet: string }[] = [];
  const found: { start: number; end: number; markerStart: number; markerEnd: number; textStart: number; task: boolean; checked: boolean; bullet: string; prefix: string; id?: string }[] = [];
  let offset = 0, fence: { ch: string; size: number } | null = null, comment = false;
  let listIndent: number | null = null;
  for (const piece of body.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const line = piece.replace(/[\r\n]+$/, ""), text = line.replace(/^[ \t]*/, ""), indent = indentationColumns(line);
    const run = /^(`{3,}|~{3,})(.*)$/.exec(text);
    if (run && (fence || indent <= 3 || (listIndent !== null && indent > listIndent))) {
      if (!fence) fence = { ch: run[1][0], size: run[1].length };
      else if (fence.ch === run[1][0] && run[1].length >= fence.size && !run[2].trim()) fence = null;
      offset += piece.length; continue;
    }
    if (fence) { offset += piece.length; continue; }
    if (text.startsWith(">")) { listIndent = null; offset += piece.length; continue; }
    const list = /^([ \t]*)([-+*]|\d+[.)])([ \t]+)/.exec(line);
    const code = indent >= 4 && (listIndent === null || indent <= listIndent);
    if (text && indent === 0 && !list) listIndent = null;
    if (code) { offset += piece.length; continue; }
    if (list) listIndent = indent;
    if (list && !comment) lines.push({ start: offset, end: offset + line.length, prefix: list[0], bullet: list[2] });
    if (list && !comment) {
      const at = list[0].length, rest = line.slice(at);
      const marker = /^(\((?: |x|X)?\)|\[(?: |x|X)?\])([ \t]+)(.+)$/.exec(rest);
      if (marker) {
        const anchor = /<!-- threestrands-task:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}) -->$/i.exec(line);
        const taskText = marker[3].replace(/\s*<!-- threestrands-task:[^\n]* -->$/, "").trim();
        if (taskText) found.push({ start: offset, end: offset + line.length, markerStart: offset + at, markerEnd: offset + at + marker[1].length, textStart: offset + at + marker[1].length + marker[2].length, task: marker[1][0] === "(", checked: /[xX]/.test(marker[1]), bullet: list[2], prefix: list[0], id: anchor?.[1] });
      }
    }
    for (let i = 0; i < line.length;) {
      if (comment) { const end = line.indexOf("-->", i); if (end < 0) break; i = end + 3; comment = false; }
      else if (line.startsWith("<!--", i)) { comment = true; i += 4; }
      else if (line[i] === "\\") i += 2;
      else if (line[i] === "`") { const n = /^`+/.exec(line.slice(i))![0].length; const close = line.indexOf("`".repeat(n), i + n); i = close < 0 ? line.length : close + n; }
      else i++;
    }
    offset += piece.length;
  }
  return { items: found, lines };
}
const MARKER_UUID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const MARKER_DATE = "\\d{4}-\\d{2}-\\d{2}";
/** One inert metadata comment the app writes at the end of a list item: a task
 * id, a plan decision (`daily_plan.rs`), or the follow-through section marker
 * (`meeting_follow_through.rs`). Anything else stays authored content. */
const MARKER = `<!-- (?:threestrands-task:${MARKER_UUID}|threestrands-plan:(?:carried ${MARKER_DATE}|deferred ${MARKER_DATE}|dropped)|threestrands-follow-through) -->`;
const markerSuffix = new RegExp(`((?: ${MARKER})+)$`, "i");
const markerBlock = new RegExp(`^(?:\\r?\\n)[ \\t]*(?:\\r?\\n[ \\t]*)?(${MARKER}(?: ${MARKER})*)(?=[ \\t]*(?:\\r?\\n|$))`, "i");
const planDate = (run: string) => [...run.matchAll(new RegExp(`threestrands-plan:\\w+ (${MARKER_DATE})`, "gi"))].every((m) => !Number.isNaN(Date.parse(`${m[1]}T00:00:00Z`)));
const endsWithMarker = (line: string) => markerSuffix.test(line) && planDate(markerSuffix.exec(line)![1]);

/** Meowdown's + [ ] round tasks are an editor representation. The persisted
 * grammar is ( ); standard Markdown [ ] remains a square checklist. */
export function toEditorMarkdown(body: string): string {
  // Inert metadata comments trailing a list item (task id, plan decision,
  // follow-through marker) move onto their own indented line: a block comment
  // is an invisible atom in meowdown, while an inline comment would show as
  // prose and stop the rest of the line from rendering. The move is reversible
  // by `fromEditorMarkdown`; only an exactly spaced, validated run moves.
  for (const item of scanListItems(body).lines.reverse()) {
    const line = body.slice(item.start, item.end);
    let run = markerSuffix.exec(line);
    if (run && !planDate(run[1])) run = null;
    // Legacy leniency: a lone task id after any spacing was always hidden.
    const lone = new RegExp(`[ \\t]*<!-- threestrands-task:${MARKER_UUID} -->$`, "i").exec(line);
    const at = run && !/[ \t]/.test(line[run.index - 1] ?? " ") ? run.index : lone && !run ? lone.index : -1;
    const text = at < 0 ? "" : line.slice(item.prefix.length, at).replace(/^(?:\((?: |x|X)?\)|\[(?: |x|X)?\])[ \t]*/, "");
    if (at < 0 || !text.trim()) continue;
    const indent = item.prefix.replace(/[^ \t]/g, " ");
    body = body.slice(0, item.start + at) + `\n\n${indent}${line.slice(at).trim()}` + body.slice(item.end);
  }
  for (const item of noteItemRanges(body).reverse()) {
    const state = item.checked ? "x" : " ";
    if (item.task) body = body.slice(0, item.start) + item.prefix.replace(item.bullet, "+") + `[${state}]` + body.slice(item.markerEnd);
    else {
      body = body.slice(0, item.markerStart) + `[${state}]` + body.slice(item.markerEnd);
      if (item.bullet === "+") body = body.slice(0, item.start) + item.prefix.replace("+", "-") + body.slice(item.start + item.prefix.length);
    }
  }
  return body;
}
/** A list item with a metadata block holds two blocks, so the editor writes
 * its whole list loose (a blank line between every item). Once the blocks are
 * folded back, take those blank lines out again where the saved note had none
 * between the same items, so an untouched note saves byte for byte. */
function restoreTightLists(body: string, original: string): string {
  const gaps = (text: string) => {
    const lines = scanListItems(text).lines;
    return { lines, gaps: lines.slice(1).map((next, i) => text.slice(lines[i].end, next.start)) };
  };
  const now = gaps(body), before = gaps(original);
  const aligned = now.lines.length === before.lines.length;
  const originalBlank = before.gaps.some((gap) => /^\r?\n(?:[ \t]*\r?\n)+$/.test(gap));
  for (let i = now.gaps.length - 1; i >= 0; i--) {
    if (!/^\r?\n(?:[ \t]*\r?\n)+$/.test(now.gaps[i])) continue;
    const tight = aligned ? /^\r?\n$/.test(before.gaps[i]) : !originalBlank;
    if (!tight) continue;
    const break_ = /^\r?\n/.exec(now.gaps[i])![0];
    body = body.slice(0, now.lines[i].end) + break_ + body.slice(now.lines[i + 1].start);
  }
  return body;
}
export function fromEditorMarkdown(body: string, original = ""): string {
  // Fold only a validated, adjacent metadata block belonging to a list
  // item. Other comments remain authored content.
  let folded = false;
  for (const item of scanListItems(body).lines.reverse()) {
    if (endsWithMarker(body.slice(item.start, item.end))) continue;
    const following = body.slice(item.end);
    const block = markerBlock.exec(following);
    if (block && planDate(block[1])) { body = body.slice(0, item.end) + ` ${block[1]}` + following.slice(block[0].length); folded = true; }
  }
  if (folded) body = restoreTightLists(body, original);
  const source = noteItemRanges(original);
  const originalText = (item: ReturnType<typeof noteItemRanges>[number], value: string) => value.slice(item.textStart, item.end).replace(/<!--.*?-->/g, "").trim();
  const ranges = noteItemRanges(body);
  for (const item of [...ranges].reverse()) {
    const round = item.task || item.bullet === "+";
    const previous = source.find((other) => item.id ? other.id === item.id : originalText(item, body) === originalText(other, original));
    const bullet = previous && (previous.task === round || /^\d/.test(previous.bullet)) ? previous.bullet : "-";
    const prefix = item.prefix.replace(item.bullet, bullet);
    const rawMarker = body.slice(item.markerStart, item.markerEnd);
    let marker = round ? `(${item.checked ? rawMarker.includes("X") ? "X" : "x" : " "})` : rawMarker;
    if (previous && previous.task === round && previous.checked === item.checked) marker = original.slice(previous.markerStart, previous.markerEnd);
    body = body.slice(0, item.start) + prefix + marker + body.slice(item.markerEnd);
  }
  return body;
}

function labelEnds(line: string): Map<number, number> {
  const ends = new Map<number, number>(), stack: number[] = [];
  for (let i = 0; i < line.length; i++) {
    if (line[i] === "\\") { i++; continue; }
    if (line[i] === "[") stack.push(i);
    else if (line[i] === "]") { const start = stack.pop(); if (start !== undefined) ends.set(start, i); }
  }
  return ends;
}

/** Entity-link counterpart of the native source scanner, used by the demo.
 * It never resolves or fetches a target. */
export function extractNoteEntityLinks(body: string): { links: EntityLink[]; unclosedFence: boolean } {
  const links: EntityLink[] = [];
  let quoteFence: { ch: string; size: number } | null = null;
  let offset = 0, fence: { ch: string; size: number } | null = null, comment = false, listIndent: number | null = null;
  for (const piece of body.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const line = piece.replace(/[\r\n]+$/, ""), text = line.replace(/^[ \t]*/, ""), indent = indentationColumns(line);
    const run = /^(`{3,}|~{3,})(.*)$/.exec(text);
    if (run && (fence || indent <= 3 || (listIndent !== null && indent > listIndent))) { if (!fence) fence = { ch: run[1][0], size: run[1].length }; else if (fence.ch === run[1][0] && run[1].length >= fence.size && !run[2].trim()) fence = null; offset += piece.length; continue; }
    if (fence) { offset += piece.length; continue; }
    if (text.startsWith(">")) {
      listIndent = null;
      const content = text.replace(/^(?:> ?)+/, "");
      const run = /^(`{3,}|~{3,})(.*)$/.exec(content);
      if (run) { if (!quoteFence) quoteFence = { ch: run[1][0], size: run[1].length }; else if (quoteFence.ch === run[1][0] && run[1].length >= quoteFence.size && !run[2].trim()) quoteFence = null; offset += piece.length; continue; }
      if (quoteFence || content.startsWith("    ") || content.startsWith("\t")) { offset += piece.length; continue; }
    } else quoteFence = null;
    const list = /^([ \t]*)([-+*]|\d+[.)])([ \t]+)/.exec(line);
    const code = indent >= 4 && (listIndent === null || indent <= listIndent);
    if (text && indent === 0 && !list) listIndent = null;
    if (code) { offset += piece.length; continue; }
    if (list) listIndent = indent;
    const labels = labelEnds(line);
    for (let i = 0; i < line.length;) {
      if (comment) { const end = line.indexOf("-->", i); if (end < 0) break; i = end + 3; comment = false; continue; }
      if (line.startsWith("<!--", i)) { comment = true; i += 4; continue; }
      if (line[i] === "\\") { i += 2; continue; }
      if (line[i] === "`") {
        const n = /^`+/.exec(line.slice(i))![0].length;
        let j = i + n, closed = false;
        while (j < line.length) { if (line[j] !== "`") { j++; continue; } const k = /^`+/.exec(line.slice(j))![0].length; j += k; if (k === n) { closed = true; break; } }
        i = closed ? j : i + n; continue;
      }
      if (line[i] === "[" && line[i - 1] !== "!") {
        const close = labels.get(i) ?? -1, end = close < 0 || line[close + 1] !== "(" ? -1 : line.indexOf(")", close + 2);
        if (end >= 0) {
          const href = line.slice(close + 2, end), reference = parseNoteReference(href);
          if (href.startsWith("threestrands:") && utf8(href) > MAX_NOTE_REFERENCE_BYTES) throw new Error("This note contains an item reference that is too long");
          if (reference) links.push({ reference, href: noteReferenceHref(reference), label: noteInlineText(line.slice(i + 1, close)), context: notePreview(line), start: offset + i });
          i = end + 1; continue;
        }
      }
      i++;
    }
    offset += piece.length;
  }
  if (links.length > MAX_NOTE_ENTITY_LINKS) throw new Error("This note has too many item links (maximum 500)");
  return { links, unclosedFence: fence !== null };
}
