import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { insertMeetingLine, meetingLine, meetingNoteTitle, type MeetingEntry } from "./noteMeetings";

const fixtures = JSON.parse(readFileSync("fixtures/note-meetings.json", "utf8")) as {
  insert: { name: string; body: string; expected: string | null }[];
  titles: [string, string][];
  lines: { name: string; entry: MeetingEntry; title: string | null; expected: string | null }[];
};
const lineOrNull = (entry: MeetingEntry, title: string | null) => { try { return meetingLine(entry, title); } catch { return null; } };

describe("meeting lines, shared with native", () => {
  it.each(fixtures.insert)("places the line: $name", ({ body, expected }) => expect(insertMeetingLine(body, "- NEW")).toBe(expected));
  it.each(fixtures.titles)("titles %j as %j", (name, title) => expect(meetingNoteTitle(name)).toBe(title));
  it.each(fixtures.lines)("writes the line: $name", ({ entry, title, expected }) => expect(lineOrNull(entry, title)).toBe(expected));
});
