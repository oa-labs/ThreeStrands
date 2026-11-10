import type { Note } from "./domain";
import { noteReferenceHref, type NoteReference } from "./noteEntities";

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
const escapeInline = (text: string) => text.replace(/[\r\n]/g, " ").replace(/[\\`*_[\]{}()#+.!|<>~-]/g, "\\$&");
const link = (label: string, reference: NoteReference) => `[${escapeInline(label)}](${noteReferenceHref(reference)})`;
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
  return `${line} for ${title !== null ? `[[${title}]]` : escapeInline(collapse(entry.name) || "Meeting")}`;
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

/** `body` with `line` at the end of its `## Meetings` section, or with the
 * heading and line appended; null when that point is inside unclosed code. */
export function insertMeetingLine(body: string, line: string): string | null {
  const key = MEETINGS_HEADING.toLowerCase();
  let fence: { ch: string; size: number } | null = null, offset = 0;
  let section: { at: number; afterHeading: boolean; inFence: boolean } | null = null;
  for (const piece of body.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const text = piece.replace(/[\r\n]+$/, ""), end = offset + piece.length;
    offset = end;
    const run = fenceRun(text);
    if (run) {
      if (!fence) fence = { ch: run.ch, size: run.size };
      else if (fence.ch === run.ch && run.size >= fence.size && !run.rest.trim()) fence = null;
      if (section) section = { at: end, afterHeading: false, inFence: fence !== null };
      continue;
    }
    if (!fence) {
      const found = heading(text);
      if (found && section && found.level <= 2) break;
      if (found && !section && found.level === 2 && found.key === key) { section = { at: end, afterHeading: true, inFence: false }; continue; }
    }
    if (section && text.trim()) section = { at: end, afterHeading: false, inFence: fence !== null };
  }
  if (section) {
    if (section.inFence) return null;
    const before = body.slice(0, section.at), after = body.slice(section.at);
    return `${before}${before.endsWith("\n") ? "" : "\n"}${section.afterHeading ? "\n" : ""}${line}\n${after}`;
  }
  if (fence) return null;
  const separator = !body || body.endsWith("\n\n") ? "" : body.endsWith("\n") ? "\n" : "\n\n";
  return `${body}${separator}## ${MEETINGS_HEADING}\n\n${line}\n`;
}
