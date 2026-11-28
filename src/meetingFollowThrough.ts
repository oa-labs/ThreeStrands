import type { FollowThroughItem, MeetingContext } from "./domain";
import { fenceRun, indent, isListItem, itemEnd, lines, occurrenceBlocks } from "./meetingExcerpt";
import { escapeMarkdownText, extractNoteEntityLinks, normalizeContactAddress, noteReferenceHref, noteReferenceKey, parseNoteReference, type NoteReference } from "./noteEntities";
import { insertMeetingLine } from "./noteMeetings";

/** Ends a follow-through section's line; mirrors `FOLLOW_THROUGH_MARKER`. */
export const FOLLOW_THROUGH_MARKER = "<!-- threestrands-follow-through -->";
export const FOLLOW_THROUGH_LABEL = "Follow-through";
/** Mirrors `src-tauri/src/meeting_policy.rs`. */
export const MAX_FOLLOW_THROUGH_ITEMS = 50;
export const MAX_FOLLOW_THROUGH_ITEM_CHARS = 500;
export const MAX_FOLLOW_UP_SUBJECT_CHARS = 998;
export const MAX_FOLLOW_UP_BODY_BYTES = 65_536;

export type FollowThroughSection = { date: string | null; start: number; end: number };

const collapse = (text: string) => text.split(/\s+/u).filter(Boolean).join(" ");
const isDate = (value: string) => /^\d{4}-\d{2}-\d{2}$/.test(value) && !Number.isNaN(Date.parse(`${value}T00:00:00Z`)) && new Date(`${value}T00:00:00Z`).toISOString().startsWith(value);
const MAX_FOLLOW_THROUGH_SUMMARY_CHARS = 4_000;
const MAX_RECAP_SUMMARY_WORDS = 400;

function checkedText(text: string): string {
  const out = collapse(text);
  if (!out) throw new Error("Each decision and action needs some text.");
  if ([...out].length > MAX_FOLLOW_THROUGH_ITEM_CHARS) throw new Error(`Shorten each decision and action to ${MAX_FOLLOW_THROUGH_ITEM_CHARS} characters or fewer.`);
  return out;
}

/** Each item's Markdown without a list marker; mirrors `item_lines`. */
export function itemLines(items: readonly FollowThroughItem[]): string[] {
  if (!items.length) throw new Error("Add a decision or an action to save.");
  if (items.length > MAX_FOLLOW_THROUGH_ITEMS) throw new Error(`Save at most ${MAX_FOLLOW_THROUGH_ITEMS} decisions and actions at a time.`);
  return items.map((item) => {
    switch (item.kind) {
      case "decision": return `Decision: ${escapeMarkdownText(checkedText(item.text))}`;
      case "task": return `( ) ${escapeMarkdownText(checkedText(item.text))}`;
      case "checklist": return `[ ] ${escapeMarkdownText(checkedText(item.text))}`;
      case "action": return `Action: ${escapeMarkdownText(checkedText(item.text))}`;
      case "question": return `Open question: ${escapeMarkdownText(checkedText(item.text))}`;
      case "link": {
        if (item.task.kind !== "task") throw new Error("Only a task can be linked as an action.");
        return `[${escapeMarkdownText(collapse(item.title) || "Task")}](${noteReferenceHref(item.task)})`;
      }
    }
  });
}

function escapePreservingBold(text: string): string {
  let out = "";
  let rest = text;
  while (true) {
    const open = rest.indexOf("**");
    if (open < 0) break;
    const before = rest.slice(0, open);
    const afterOpen = rest.slice(open + 2);
    const close = afterOpen.indexOf("**");
    if (close > 0) {
      const inner = afterOpen.slice(0, close);
      if (!/^\s|\s$/.test(inner)) {
        out += `${escapeMarkdownText(before)}**${escapeMarkdownText(inner)}**`;
        rest = afterOpen.slice(close + 2);
        continue;
      }
    }
    out += escapeMarkdownText(rest.slice(0, open + 2));
    rest = afterOpen;
  }
  return out + escapeMarkdownText(rest);
}

function sanitizeSummaryLine(raw: string): string {
  const escaped = escapePreservingBold(collapse(raw));
  return /^\([ xX]\)\s/.test(escaped) ? `\\${escaped}` : escaped;
}

function checkedSummaryLines(raw: string | null | undefined): string[] {
  if (!raw) return [];
  const normalized = raw.replace(/\r\n/g, "\n");
  const nonEmpty = normalized.split("\n").map((line) => line.trim()).filter(Boolean);
  if (!nonEmpty.length) return [];
  const isStructured = normalized.includes("\n")
    || nonEmpty[0].startsWith("- ")
    || nonEmpty[0].startsWith("* ")
    || nonEmpty[0].startsWith("• ")
    || nonEmpty[0].startsWith("**");
  if (!isStructured) {
    const out = collapse(normalized);
    if ([...out].length > MAX_FOLLOW_THROUGH_SUMMARY_CHARS) throw new Error(`Shorten the summary to ${MAX_FOLLOW_THROUGH_SUMMARY_CHARS} characters or fewer.`);
    if (out.split(/\s+/u).filter(Boolean).length > MAX_RECAP_SUMMARY_WORDS) throw new Error(`Shorten the summary to ${MAX_RECAP_SUMMARY_WORDS} words or fewer.`);
    return [`  - Summary: ${escapeMarkdownText(out)}`];
  }

  const cleaned = nonEmpty.map(collapse);
  const totalChars = cleaned.reduce((sum, line) => sum + [...line].length, 0) + Math.max(0, cleaned.length - 1);
  if (totalChars > MAX_FOLLOW_THROUGH_SUMMARY_CHARS) throw new Error(`Shorten the summary to ${MAX_FOLLOW_THROUGH_SUMMARY_CHARS} characters or fewer.`);
  const totalWords = cleaned.reduce((sum, line) => {
    const content = line.replace(/^(?:- |\* |• )/, "");
    return sum + content.split(/\s+/u).filter(Boolean).length;
  }, 0);
  if (totalWords > MAX_RECAP_SUMMARY_WORDS) throw new Error(`Shorten the summary to ${MAX_RECAP_SUMMARY_WORDS} words or fewer.`);

  const out: string[] = [];
  let hasParent = false;
  for (const [index, line] of cleaned.entries()) {
    const bulletMatch = /^(?:- |\* |• )(.*)$/.exec(line);
    if (bulletMatch) {
      const safe = sanitizeSummaryLine(bulletMatch[1]);
      if (!safe) continue;
      out.push(hasParent ? `    - ${safe}` : `  - ${safe}`);
    } else {
      const safe = sanitizeSummaryLine(line);
      if (!safe) continue;
      if (index === 0 && !line.startsWith("**") && !line.startsWith("Summary:") && !line.startsWith("Overview:")) {
        out.push(`  - **Overview:** ${safe}`);
      } else {
        out.push(`  - ${safe}`);
      }
      hasParent = true;
    }
  }
  return out;
}

function sectionBodyLines(summary: string | null | undefined, items: readonly FollowThroughItem[], transcript: { label: string; href: string } | null | undefined): string[] {
  if (items.length > MAX_FOLLOW_THROUGH_ITEMS) throw new Error(`Save at most ${MAX_FOLLOW_THROUGH_ITEMS} decisions and actions at a time.`);
  const linesOut: string[] = checkedSummaryLines(summary);
  if (items.length) linesOut.push(...itemLines(items).map((line) => `  - ${line}`));
  if (transcript) {
    const label = collapse(transcript.label) || "Transcript";
    linesOut.push(`  - Transcript: [${escapeMarkdownText(label)}](${transcript.href})`);
  }
  if (!linesOut.length) throw new Error("Add a decision or an action to save.");
  return linesOut;
}

/** Whether `line` starts a follow-through section; mirrors `is_section_line`. */
export function isSectionLine(line: string): boolean {
  const trimmed = line.trimEnd();
  return isListItem(line) && trimmed.endsWith(FOLLOW_THROUGH_MARKER) && !trimmed.slice(0, -FOLLOW_THROUGH_MARKER.length).endsWith("\\");
}

function sectionDate(line: string): string | null {
  const at = line.indexOf(FOLLOW_THROUGH_LABEL);
  if (at < 0) return null;
  const candidate = line.slice(at + FOLLOW_THROUGH_LABEL.length).trimStart().slice(0, 10);
  return isDate(candidate) ? candidate : null;
}

/** Follow-through sections inside this occurrence's meeting blocks; mirrors
 * `sections` in `src-tauri/src/meeting_follow_through.rs`. Both pass
 * `fixtures/meeting-follow-through.json`. Offsets are string indexes. */
export function followThroughSections(body: string, occurrence: NoteReference): FollowThroughSection[] {
  const all = lines(body);
  const out: FollowThroughSection[] = [];
  for (const block of occurrenceBlocks(body, occurrence)) {
    if (block.kind !== "excerpt") continue;
    const first = all.findIndex((line) => line.start === block.start);
    if (first < 0) continue;
    let fence: { mark: string; run: number } | null = null;
    for (let at = first; at < all.length; at++) {
      const line = all[at];
      if (line.start >= block.end) break;
      const run = fenceRun(line.text);
      if (fence) {
        if (run && run.mark === fence.mark && run.run >= fence.run && !run.trailing) fence = null;
        continue;
      }
      if (run) { fence = { mark: run.mark, run: run.run }; continue; }
      if (!isSectionLine(line.text) || out.some((section) => section.start === line.start)) continue;
      const end = itemEnd(all, at);
      if (end !== null) out.push({ date: sectionDate(line.text), start: line.start, end });
    }
  }
  return out.sort((a, b) => a.start - b.start);
}

/** The section's Markdown as it will be written, without the line break or
 * blank line that separates it from the text before it. */
export function followThroughSectionText(
  occurrence: NoteReference,
  name: string,
  date: string,
  items: readonly FollowThroughItem[],
  extras?: { summary?: string | null; transcript?: { label: string; href: string } | null },
): string {
  const label = escapeMarkdownText(collapse(name) || "Meeting");
  return [
    `- ${FOLLOW_THROUGH_LABEL} ${date}: [${label}](${noteReferenceHref(occurrence)}) ${FOLLOW_THROUGH_MARKER}`,
    ...sectionBodyLines(extras?.summary, items, extras?.transcript),
  ].join("\n");
}

/** Formats the daily-note `## Meetings` entry linking an ad-hoc meeting note. */
export function adhocDailyMeetingLine(timeLabel: string, title: string): string {
  const time = escapeMarkdownText(collapse(timeLabel) || "Meeting");
  return `- ${time} for [[${title}]]`;
}

/** Formats an ad-hoc recording's follow-through section (no calendar occurrence link). */
export function adhocFollowThroughSectionText(
  timeLabel: string,
  name: string,
  items: readonly FollowThroughItem[],
  extras?: { summary?: string | null; transcript?: { label: string; href: string } | null },
): string {
  const time = escapeMarkdownText(collapse(timeLabel) || "Meeting");
  const label = escapeMarkdownText(collapse(name) || "Meeting");
  return [
    `- ${time} for ${label} ${FOLLOW_THROUGH_MARKER}`,
    ...sectionBodyLines(extras?.summary, items, extras?.transcript),
  ].join("\n");
}

/** `body` with the section added; mirrors `insert`. */
export function insertFollowThrough(
  body: string,
  occurrence: NoteReference,
  name: string,
  date: string,
  items: readonly FollowThroughItem[],
  extras?: { summary?: string | null; transcript?: { label: string; href: string } | null },
): { body: string; start: number; end: number } {
  if (!isDate(date)) throw new Error("Invalid date");
  const section = followThroughSectionText(occurrence, name, date, items, extras);
  const all = lines(body);
  const blocks = occurrenceBlocks(body, occurrence).filter((block) => block.kind === "excerpt");
  const position = (start: number) => all.findIndex((line) => line.start === start);
  let after: number | null = null;
  for (const block of [...blocks].reverse()) {
    const at = position(block.start);
    if (at >= 0 && indent(all[at].text) === 0) { after = block.end; break; }
  }
  if (after === null && blocks.length) {
    const at = position(blocks[0].start);
    let top = -1;
    for (let index = at - 1; index >= 0; index--) if (isListItem(all[index].text) && indent(all[index].text) === 0) { top = index; break; }
    const end = at >= 0 && top >= 0 ? itemEnd(all, top) : null;
    if (end !== null && end >= blocks[0].end) after = end;
  }
  let at: number, chunk: string;
  if (after !== null) { at = after; chunk = `\n${section}`; }
  else {
    if (extractNoteEntityLinks(body).unclosedFence) throw new Error("This note has a code block that never closes. Open it before saving follow-through.");
    const separator = !body || body.endsWith("\n\n") ? "" : body.endsWith("\n") ? "\n" : "\n\n";
    at = body.length; chunk = `${separator}${section}\n`;
  }
  const next = body.slice(0, at) + chunk + body.slice(at);
  if (followThroughSections(next, occurrence).length !== followThroughSections(body, occurrence).length + 1) throw new Error("This note's layout changed. Open it before saving follow-through.");
  return { body: next, start: at, end: at + chunk.length };
}

/** Inserts an ad-hoc recording's follow-through section under `## Meetings` in a daily note. */
export function insertAdhocFollowThrough(
  body: string,
  timeLabel: string,
  name: string,
  date: string,
  items: readonly FollowThroughItem[],
  extras?: { summary?: string | null; transcript?: { label: string; href: string } | null },
): { body: string; start: number; end: number } {
  if (!isDate(date)) throw new Error("Invalid date");
  const section = adhocFollowThroughSectionText(timeLabel, name, items, extras);
  const next = insertMeetingLine(body, section);
  if (next === null) throw new Error("This note has a code block that never closes. Open it before saving follow-through.");
  let start = 0;
  while (start < body.length && start < next.length && body[start] === next[start]) start += 1;
  let suffix = 0;
  while (
    suffix < body.length - start
    && suffix < next.length - start
    && body[body.length - 1 - suffix] === next[next.length - 1 - suffix]
  ) suffix += 1;
  const end = next.length - suffix;
  if (end < start || next.slice(0, start) !== body.slice(0, start) || next.slice(end) !== body.slice(start)) {
    throw new Error("This note's layout changed. Open it before saving follow-through.");
  }
  return { body: next, start, end };
}

/** Inserts an ad-hoc recording's follow-through section directly into a dedicated meeting note. */
export function insertAdhocFollowThroughInNote(
  body: string,
  timeLabel: string,
  name: string,
  date: string,
  items: readonly FollowThroughItem[],
  extras?: { summary?: string | null; transcript?: { label: string; href: string } | null },
): { body: string; start: number; end: number } {
  if (!isDate(date)) throw new Error("Invalid date");
  const section = adhocFollowThroughSectionText(timeLabel, name, items, extras);
  if (extractNoteEntityLinks(body).unclosedFence) throw new Error("This note has a code block that never closes. Open it before saving follow-through.");
  const separator = !body || body.endsWith("\n\n") ? "" : body.endsWith("\n") ? "\n" : "\n\n";
  const at = body.length;
  const chunk = `${separator}${section}\n`;
  return { body: `${body}${chunk}`, start: at, end: at + chunk.length };
}

/** A local date as `YYYY-MM-DD`. */
export function localDay(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Local state below belongs to this device only: none of it syncs or goes
 * into a settings export, and the note content stays the record. */
function readList<T>(key: string, valid: (value: unknown) => value is T): T[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return Array.isArray(stored) ? stored.filter(valid) : [];
  } catch {
    return [];
  }
}
function writeList(key: string, values: unknown[]) {
  try { localStorage.setItem(key, JSON.stringify(values)); } catch { /* Applies until reload. */ }
}


export type ReviewEvidence = { segmentId: number; startMs: number; speaker: "you" | "others" | null; speakerLabel: string; text: string };
/** An item being edited in the review: its kind and text, or the task it links. */
export type ReviewItem = { id: string; suggested?: boolean; evidence?: ReviewEvidence[] } & FollowThroughItem;
/** A review in progress, kept on this device until it is saved or discarded. */
export type FollowThroughRecovery = {
  reference: NoteReference | null;
  reviewId: string;
  /** Chosen before the first save attempt, so a retry after a restart replays nothing. */
  operationId: string;
  destinationNoteId: string | null;
  summary?: string;
  attachTranscript?: boolean;
  recordingId?: string | null;
  adhocTitle?: string;
  items: ReviewItem[];
  updatedAt: number;
};
const RECOVERY_KEY = "threestrands.meetings.followThroughReviews";
/** Reviews kept and how long after their last edit, as `meeting_policy.rs` records. */
export const MAX_FOLLOW_THROUGH_REVIEWS = 50;
export const FOLLOW_THROUGH_REVIEW_RETENTION_MS = 14 * 86_400_000;

const isRecovery = (value: unknown): value is FollowThroughRecovery => typeof value === "object" && value !== null
  && typeof (value as FollowThroughRecovery).reviewId === "string" && typeof (value as FollowThroughRecovery).operationId === "string"
  && Array.isArray((value as FollowThroughRecovery).items) && typeof (value as FollowThroughRecovery).updatedAt === "number"
  && ((value as FollowThroughRecovery).reference === null || typeof (value as FollowThroughRecovery).reference === "object");

function recoveries(now: number): FollowThroughRecovery[] {
  return readList(RECOVERY_KEY, isRecovery).filter((record) => now - record.updatedAt <= FOLLOW_THROUGH_REVIEW_RETENTION_MS);
}
/** The review in progress for this occurrence (or recording), if one was kept. */
export function readFollowThroughRecovery(reference: NoteReference | null, now = Date.now(), recordingId: string | null = null): FollowThroughRecovery | null {
  if (recordingId) {
    const byRec = recoveries(now).filter((record) => record.recordingId === recordingId).sort((a, b) => b.updatedAt - a.updatedAt)[0];
    if (byRec) return byRec;
  }
  if (!reference) return null;
  const key = noteReferenceKey(reference);
  return recoveries(now).filter((record) => record.reference && noteReferenceKey(record.reference) === key).sort((a, b) => b.updatedAt - a.updatedAt)[0] ?? null;
}
/** Keeps a review's edits; the oldest and expired reviews go first. */
export function saveFollowThroughRecovery(record: FollowThroughRecovery, now = Date.now()) {
  const others = recoveries(now).filter((other) => other.reviewId !== record.reviewId);
  writeList(RECOVERY_KEY, [...others, { ...record, updatedAt: now }].sort((a, b) => a.updatedAt - b.updatedAt).slice(-MAX_FOLLOW_THROUGH_REVIEWS));
}
export function discardFollowThroughRecovery(reviewId: string, now = Date.now()) {
  writeList(RECOVERY_KEY, recoveries(now).filter((record) => record.reviewId !== reviewId));
}

const HANDOFF_KEY = "threestrands.meetings.followUpHandoffs";
/** The follow-up draft handoff for each occurrence, so Draft Follow-up resumes it. */
type HandoffRecord = { key: string; handoffId: string; at: number };
const isHandoff = (value: unknown): value is HandoffRecord => typeof value === "object" && value !== null && typeof (value as HandoffRecord).key === "string" && typeof (value as HandoffRecord).handoffId === "string";
export function readFollowUpHandoff(reference: NoteReference): string | null {
  const key = noteReferenceKey(reference);
  return readList(HANDOFF_KEY, isHandoff).find((record) => record.key === key)?.handoffId ?? null;
}
export function rememberFollowUpHandoff(reference: NoteReference, handoffId: string | null) {
  const key = noteReferenceKey(reference);
  const others = readList(HANDOFF_KEY, isHandoff).filter((record) => record.key !== key);
  writeList(HANDOFF_KEY, [...others, ...(handoffId ? [{ key, handoffId, at: Date.now() }] : [])].slice(-MAX_FOLLOW_THROUGH_REVIEWS));
}

/** The attendees a follow-up starts addressed to: the event's other people,
 * without the user's own mail identities or calendar addresses. */
export function followUpRecipients(context: MeetingContext | null, ownAddresses: readonly string[]): string[] {
  const own = new Set(ownAddresses.map(normalizeContactAddress));
  return [...new Set((context?.event?.attendees ?? []).map(normalizeContactAddress))].filter((address) => address.includes("@") && !own.has(address));
}

/** The follow-up's starting text, assembled locally from the reviewed items. */
export function followUpBody(title: string, items: readonly FollowThroughItem[]): string {
  const text = (item: FollowThroughItem) => collapse(item.kind === "link" ? item.title : item.text);
  const decisions = items.filter((item) => item.kind === "decision").map(text).filter(Boolean);
  const actions = items.filter((item) => item.kind !== "decision").map(text).filter(Boolean);
  const parts = [`Thanks for joining ${collapse(title) || "the meeting"}. A quick summary:`];
  if (decisions.length) parts.push(["Decisions:", ...decisions.map((line) => `- ${line}`)].join("\n"));
  if (actions.length) parts.push(["Next steps:", ...actions.map((line) => `- ${line}`)].join("\n"));
  return `${parts.join("\n\n")}\n`;
}

const unescape = (text: string) => text.replace(/\\(.)/g, "$1");

/** The decisions and actions in a saved section's text, for drafting a
 * follow-up from it: the reverse of `itemLines`, reading the nested items
 * only. Task anchors and other comments are left out. */
export function itemsFromSection(text: string): FollowThroughItem[] {
  const items: FollowThroughItem[] = [];
  for (const raw of text.split("\n").slice(1)) {
    const match = /^[ \t]+[-+*][ \t]+(.*)$/.exec(raw);
    if (!match) continue;
    const line = match[1].replace(/\s*<!--[\s\S]*?-->\s*$/, "").trim();
    const link = /^\[((?:\\.|[^\]\\])*)\]\((threestrands:task\/[^)]+)\)$/.exec(line);
    const linked = link ? parseNoteReference(link[2]) : null;
    if (link && linked) items.push({ kind: "link", task: linked, title: unescape(link[1]) });
    else if (/^\([ xX]\)\s/.test(line)) items.push({ kind: "task", text: unescape(line.slice(4)) });
    else if (/^\[[ xX]\]\s/.test(line)) items.push({ kind: "checklist", text: unescape(line.slice(4)) });
    else if (line.startsWith("Decision: ")) items.push({ kind: "decision", text: unescape(line.slice(10)) });
    else if (line.startsWith("Action: ")) items.push({ kind: "action", text: unescape(line.slice(8)) });
    else if (line.startsWith("Open question: ")) items.push({ kind: "question", text: unescape(line.slice(15)) });
  }
  return items;
}
