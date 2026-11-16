import { escapeMarkdownText, extractNoteEntityLinks, noteItemRanges, noteReferenceKey, type NoteReference } from "./noteEntities";
import { noteInlineText } from "./notePreview";

/** `excerpt` is a list item linking the meeting with its nested content;
 * `reference` is only the line of a link whose surrounding text isn't a
 * bounded meeting block. */
export type MeetingBlock = { kind: "excerpt" | "reference"; start: number; end: number };

export type Line = { start: number; text: string };

export function lines(body: string): Line[] {
  const out: Line[] = [];
  let offset = 0;
  for (const piece of body.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    out.push({ start: offset, text: piece.replace(/[\r\n]+$/, "") });
    offset += piece.length;
  }
  return out;
}

export const lineEnd = (line: Line) => line.start + line.text.length;

export function indent(text: string): number {
  return [...(/^[ \t]*/.exec(text)?.[0] ?? "")].reduce((column, ch) => ch === "\t" ? column + 4 - column % 4 : column + 1, 0);
}

export const isListItem = (text: string) => /^[ \t]*(?:[-+*]|\d+[.)])[ \t]/.test(text);

export function fenceRun(text: string): { mark: string; run: number; trailing: boolean } | null {
  const match = /^[ \t]*(`{3,}|~{3,})(.*)$/.exec(text);
  return match ? { mark: match[1][0], run: match[1].length, trailing: match[2].trim() !== "" } : null;
}

/** The end of the list item's block on line `at`, or null when a nested
 * fence never closes. */
export function itemEnd(all: Line[], at: number): number | null {
  const itemIndent = indent(all[at].text);
  let end = lineEnd(all[at]);
  let fence: { mark: string; run: number } | null = null;
  for (const line of all.slice(at + 1)) {
    if (fence) {
      const run = fenceRun(line.text);
      if (run && run.mark === fence.mark && run.run >= fence.run && !run.trailing) fence = null;
      end = lineEnd(line);
      continue;
    }
    if (!line.text.trim()) continue;
    if (indent(line.text) <= itemIndent) break;
    const run = fenceRun(line.text);
    if (run) fence = { mark: run.mark, run: run.run };
    end = lineEnd(line);
  }
  return fence ? null : end;
}

/** Meeting blocks for links `isTarget` accepts, in text order: at most one
 * per line, and none for a link inside an earlier block. Mirrors
 * `meeting_blocks` in `src-tauri/src/meeting_excerpt.rs`; both pass
 * `fixtures/meeting-excerpts.json`. Offsets are string indexes. */
export function meetingBlocks(body: string, isTarget: (reference: NoteReference) => boolean): MeetingBlock[] {
  const all = lines(body);
  const blocks: MeetingBlock[] = [];
  for (const link of extractNoteEntityLinks(body).links.filter((link) => isTarget(link.reference))) {
    if (blocks.some((block) => block.start <= link.start && link.start < block.end)) continue;
    const at = all.findIndex((line) => line.start <= link.start && link.start < lineEnd(line));
    if (at < 0) continue;
    const line = all[at];
    const quoted = line.text.trimStart().startsWith(">");
    const end = !quoted && isListItem(line.text) ? itemEnd(all, at) : null;
    blocks.push(end === null ? { kind: "reference", start: line.start, end: lineEnd(line) } : { kind: "excerpt", start: line.start, end });
  }
  return blocks;
}

/** Blocks naming exactly this occurrence; a series link or another occurrence doesn't count. */
export function occurrenceBlocks(body: string, occurrence: NoteReference): MeetingBlock[] {
  const key = noteReferenceKey(occurrence);
  return meetingBlocks(body, (reference) => reference.kind === "event" && noteReferenceKey(reference) === key);
}

/** `text` cut to at most `limit` UTF-8 bytes at a character boundary. */
export function boundedText(text: string, limit: number): { text: string; truncated: boolean } {
  const encoder = new TextEncoder();
  if (encoder.encode(text).length <= limit) return { text, truncated: false };
  let out = "", used = 0;
  for (const ch of text) {
    const size = encoder.encode(ch).length;
    if (used + size > limit) break;
    out += ch;
    used += size;
  }
  return { text: out, truncated: true };
}

/** The words a prep task starts with; mirrors `PREP_TASK_PREFIX` natively. */
export const PREP_TASK_PREFIX = "Prepare for ";

/** `body` with a round prep task nested under the first list item linking
 * `occurrence`, `"existing"` when that item already holds one, or
 * `"noMeetingLine"`. Mirrors `insert_prep_task` in
 * `src-tauri/src/meeting_excerpt.rs`. */
export function insertPrepTask(body: string, occurrence: NoteReference, name: string): string | "existing" | "noMeetingLine" {
  const block = occurrenceBlocks(body, occurrence).find((candidate) => candidate.kind === "excerpt");
  if (!block) return "noMeetingLine";
  const prefix = PREP_TASK_PREFIX.trimEnd().toLowerCase();
  const items = noteItemRanges(body);
  const titleOf = (item: (typeof items)[number]) => noteInlineText(body.slice(item.textStart, item.end).replace(/<!--.*?-->/g, "").trim()).toLowerCase();
  if (items.some((item) => item.task && block.start < item.start && item.start < block.end && titleOf(item).startsWith(prefix))) return "existing";
  const itemLine = body.slice(block.start).split("\n")[0];
  const match = /^([ \t]*)(\S+)([ \t]+)/.exec(itemLine);
  if (!match) throw new Error("Invalid meeting line");
  const indent = `${match[1]}${" ".repeat(match[2].length + match[3].length)}`;
  const title = escapeMarkdownText(name.split(/\s+/u).filter(Boolean).join(" ") || "Meeting");
  return `${body.slice(0, block.end)}\n${indent}- ( ) ${PREP_TASK_PREFIX}${title}${body.slice(block.end)}`;
}
