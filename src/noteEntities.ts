import { notePreview, noteInlineText } from "./notePreview";
import type { Note, NoteBacklink, ThreadTask } from "./domain";

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
export type NoteCapture = { note: Note; added: boolean; previousBody: string };
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
export function markdownEntityLink(label: string, reference: NoteReference): string {
  return `[${label.replace(/[\r\n]/g, " ").replace(/[\\`*_[\]{}()#+.!|<>~-]/g, "\\$&")}](${noteReferenceHref(reference)})`;
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
export function noteItemRanges(body: string) {
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
  return found;
}
/** Meowdown's + [ ] round tasks are an editor representation. The persisted
 * grammar is ( ); standard Markdown [ ] remains a square checklist. */
export function toEditorMarkdown(body: string): string {
  for (const item of noteItemRanges(body).reverse()) {
    const state = item.checked ? "x" : " ";
    // A block comment is an invisible atom in meowdown; an inline comment
    // would expose the durable task id as prose. Keep this view transform
    // reversible, including metadata retained on converted checklists.
    if (item.id) {
      const line = body.slice(item.start, item.end);
      const anchor = /[ \t]*<!-- threestrands-task:[^\n]* -->$/.exec(line);
      if (anchor) {
        const indent = /^\s*/.exec(item.prefix)![0];
        body = body.slice(0, item.start + anchor.index) + `\n\n${indent}  ${anchor[0].trim()}` + body.slice(item.end);
      }
    }
    if (item.task) body = body.slice(0, item.start) + item.prefix.replace(item.bullet, "+") + `[${state}]` + body.slice(item.markerEnd);
    else {
      body = body.slice(0, item.markerStart) + `[${state}]` + body.slice(item.markerEnd);
      if (item.bullet === "+") body = body.slice(0, item.start) + item.prefix.replace("+", "-") + body.slice(item.start + item.prefix.length);
    }
  }
  return body;
}
export function fromEditorMarkdown(body: string, original = ""): string {
  // Fold only a validated, adjacent metadata block belonging to a list
  // item. Other comments remain authored content.
  for (const item of noteItemRanges(body).reverse()) {
    if (item.id) continue;
    const following = body.slice(item.end);
    const anchor = /^(?:\r?\n)[ \t]*(?:\r?\n[ \t]*)?<!-- threestrands-task:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}) -->/i.exec(following);
    if (anchor) body = body.slice(0, item.end) + ` <!-- threestrands-task:${anchor[1]} -->` + following.slice(anchor[0].length);
  }
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
