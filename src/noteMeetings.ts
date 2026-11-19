import type { Note } from "./domain";
import { escapeMarkdownText, markdownEntityLink, parseNoteReference, type NoteReference } from "./noteEntities";

/** Mirrors `src-tauri/src/note_meetings.rs` for the demo client; both are held
 * to `fixtures/note-meetings.json`. */
export const MEETINGS_HEADING = "Meetings";
export const MAX_MEETING_ATTENDEES = 50;
export const MAX_MEETING_TIME_CHARS = 32;
const MAX_NOTE_TITLE_CHARS = 200;

export type MeetingAttendee = { label: string; reference: NoteReference };
/** What the user chose in the Add Note dialog. */
export type MeetingEntry = {
  event: NoteReference;
  /** The start time as the agenda shows it, or "All day". */
  time: string;
  attendees: MeetingAttendee[];
  /** The meeting name, which may differ from the event's title. */
  name: string;
  /** Link a note named after the meeting rather than writing its name. */
  linkNote: boolean;
};
export type MeetingCapture = { note: Note; added: boolean; previousBody: string; meetingNote: Note | null; createdMeetingNote: boolean };

const collapse = (text: string) => text.split(/\s+/u).filter(Boolean).join(" ");
const link = markdownEntityLink;
const isDateShape = (value: string) => /^\d{4}-\d{2}-\d{2}$/.test(value);

/** The title of the note a meeting links: the name made valid as a title. */
export function meetingNoteTitle(name: string): string {
  const replaced = name.replace(/[[\]|]/g, "-").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ");
  const title = [...collapse(replaced)].slice(0, MAX_NOTE_TITLE_CHARS).join("").trimEnd();
  if (!title) return "Meeting";
  return isDateShape(title) ? `${title} (meeting)` : title;
}

/** The meeting's line; `title` is the meeting note's title when the name links one. */
export function meetingLine(entry: MeetingEntry, title: string | null): string {
  if (entry.event.kind !== "event") throw new Error("Invalid event reference");
  const time = collapse(entry.time);
  if (!time || [...time].length > MAX_MEETING_TIME_CHARS) throw new Error("Invalid meeting time");
  if (entry.attendees.length > MAX_MEETING_ATTENDEES) throw new Error(`A meeting line can name at most ${MAX_MEETING_ATTENDEES} attendees`);
  let line = `- ${link(time, entry.event)}`;
  const people = entry.attendees.map((attendee) => {
    if (attendee.reference.kind !== "contact") throw new Error("Invalid attendee");
    return link(collapse(attendee.label) || attendee.reference.id, attendee.reference);
  });
  if (people.length) line += ` met with ${people.join(", ")}`;
  return `${line} for ${title !== null ? `[[${title}]]` : escapeMarkdownText(collapse(entry.name) || "Meeting")}`;
}

function fenceRun(line: string) {
  const match = /^( {0,3})(`{3,}|~{3,})(.*)$/.exec(line);
  return match ? { ch: match[2]![0]!, size: match[2]!.length, rest: match[3]! } : null;
}
function heading(line: string) {
  const match = /^ {0,3}(#{1,6})(?:[ \t](.*))?$/.exec(line);
  if (!match) return null;
  let content = (match[2] ?? "").trim();
  const without = content.replace(/#+$/, "");
  if (without.length < content.length && (!without || /[ \t]$/.test(without))) content = without.trimEnd();
  return { level: match[1]!.length, key: collapse(content).toLowerCase() };
}

/** Only top-level meeting entries participate, so insertion never separates
 * a meeting from its nested notes. Unknown time labels retain append behavior. */
function meetingTimeOrder(line: string): number | null {
  if (!line.startsWith("- ")) return null;
  let label: string;
  const linked = /^- \[([^\]]+)\]\(([^)]+)\)(?: |$)/.exec(line);
  if (linked) {
    if (parseNoteReference(linked[2])?.kind !== "event") return null;
    label = linked[1];
  } else {
    label = line.slice(2).split(/ met with | for /, 1)[0];
  }
  label = collapse(label).toLowerCase();
  if (label === "all day") return -1;
  const clock = /^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/.exec(label);
  if (!clock || (!clock[2] && !clock[3])) return null;
  const hour = Number(clock[1]), minute = Number(clock[2] ?? 0), period = clock[3];
  if (minute > 59 || (period ? hour < 1 || hour > 12 : hour > 23)) return null;
  return (period ? hour % 12 + (period === "pm" ? 12 : 0) : hour) * 60 + minute;
}

/** Insert before the first later top-level meeting in `## Meetings`, keeping
 * equal times stable and all-day entries first. Otherwise append to the section
 * (or add it); null when the section ends inside unclosed code. */
export function insertMeetingLine(body: string, line: string): string | null {
  const key = MEETINGS_HEADING.toLowerCase();
  const time = meetingTimeOrder(line);
  let orderedAt: number | null = null;
  let comment = false;
  let fence: { ch: string; size: number } | null = null, offset = 0;
  let section: { at: number; afterHeading: boolean; inFence: boolean } | null = null;
  for (const piece of body.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const text = piece.replace(/[\r\n]+$/, ""), end = offset + piece.length;
    offset = end;
    const run = comment ? null : fenceRun(text);
    if (run) {
      if (!fence) fence = { ch: run.ch, size: run.size };
      else if (fence.ch === run.ch && run.size >= fence.size && !run.rest.trim()) fence = null;
      if (section) section = { at: end, afterHeading: false, inFence: fence !== null };
      continue;
    }
    if (!fence && !comment) {
      const found = heading(text);
      if (found && section && found.level <= 2) break;
      if (found && !section && found.level === 2 && found.key === key) { section = { at: end, afterHeading: true, inFence: false }; continue; }
      if (section && time !== null && orderedAt === null) {
        const existingTime = meetingTimeOrder(text);
        if (existingTime !== null && existingTime > time) orderedAt = end - piece.length;
      }
    }
    if (!fence) {
      for (let i = 0; i < text.length;) {
        if (comment) { const end = text.indexOf("-->", i); if (end < 0) break; i = end + 3; comment = false; }
        else if (text.startsWith("<!--", i)) { comment = true; i += 4; }
        else if (text[i] === "\\") i += 2;
        else if (text[i] === "`") { const size = /^`+/.exec(text.slice(i))![0].length; const close = text.indexOf("`".repeat(size), i + size); i = close < 0 ? text.length : close + size; }
        else i++;
      }
    }
    if (section && text.trim()) section = { at: end, afterHeading: false, inFence: fence !== null };
  }
  if (section) {
    if (section.inFence) return null;
    const at = orderedAt ?? section.at;
    const before = body.slice(0, at), after = body.slice(at);
    return `${before}${before.endsWith("\n") ? "" : "\n"}${orderedAt === null && section.afterHeading ? "\n" : ""}${line}\n${after}`;
  }
  if (fence) return null;
  const separator = !body || body.endsWith("\n\n") ? "" : body.endsWith("\n") ? "\n" : "\n\n";
  return `${body}${separator}## ${MEETINGS_HEADING}\n\n${line}\n`;
}
