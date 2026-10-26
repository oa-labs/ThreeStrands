import { describe, expect, it } from "vitest";
import { formatClockTime } from "./agendaEvents";
import type { ScheduleEvent } from "./domain";
import { agendaConflicts, dayShape, FREE_STRETCH_MINUTES, meetingsWith, mentionsRecording, sharedMeetingsNote } from "./todayInsights";

const now = new Date(2026, 9, 26, 10, 0);
const at = (hours: number, minutes = 0) => new Date(2026, 9, 26, hours, minutes);
const clock = (hours: number, minutes = 0) => formatClockTime(at(hours, minutes));

function meeting(id: string, start: Date, end: Date, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent {
  return { id, accountId: "a", title: id, allDay: false, start: start.toISOString(), end: end.toISOString(), ...overrides };
}

describe("agendaConflicts", () => {
  it("pairs overlapping meetings both ways and ignores ones that only touch", () => {
    const cs = meeting("CS Updates", at(16), at(16, 20));
    const ramp = meeting("Ramp AI", at(16), at(17));
    const after = meeting("After", at(17), at(18));
    const conflicts = agendaConflicts([cs, ramp, after]);
    expect(conflicts.get(cs)).toEqual([ramp]);
    expect(conflicts.get(ramp)).toEqual([cs]);
    expect(conflicts.has(after)).toBe(false);
  });

  it("never counts declined or all-day events as a clash", () => {
    const going = meeting("Going", at(14), at(15));
    const declined = meeting("Declined", at(14), at(15), { responseStatus: "declined" });
    const holiday = { ...meeting("Holiday", at(0), at(23, 59)), allDay: true, start: "2026-10-26", end: "2026-10-27" };
    expect(agendaConflicts([going, declined, holiday]).size).toBe(0);
  });
});

describe("mentionsRecording", () => {
  it("reads a recording from the title or description, and not a negation", () => {
    expect(mentionsRecording(meeting("CS", at(16), at(17), { description: "This session will be recorded for the team." }))).toBe(true);
    expect(mentionsRecording(meeting("All hands (session is recorded)", at(16), at(17)))).toBe(true);
    expect(mentionsRecording(meeting("Talk", at(16), at(17), { description: "The recording will be shared afterwards." }))).toBe(true);
    expect(mentionsRecording(meeting("Board", at(16), at(17), { description: "This meeting will not be recorded." }))).toBe(false);
    expect(mentionsRecording(meeting("Studio", at(16), at(17), { description: "Bring your recording gear." }))).toBe(false);
  });
});

describe("dayShape", () => {
  it("names a free stretch, a double-booking with what is recorded, and when the day ends", () => {
    expect(dayShape([
      meeting("Marketing", at(14), at(14, 25)),
      meeting("CS Updates", at(16), at(16, 20), { description: "The session is recorded." }),
      meeting("Ramp AI", at(16), at(17)),
    ], now)).toBe(`Free until ${clock(14)} · ${clock(16)} double-booked (CS Updates is recorded) · Last meeting ends ${clock(17)}`);
  });

  it("calls a gap free time from the threshold up", () => {
    const startingIn = (minutes: number) => [meeting("Next", new Date(now.getTime() + minutes * 60_000), at(18))];
    expect(dayShape(startingIn(FREE_STRETCH_MINUTES - 1), now)).not.toMatch(/^Free until/);
    expect(dayShape(startingIn(FREE_STRETCH_MINUTES), now)).toMatch(/^Free until/);
    expect(dayShape(startingIn(FREE_STRETCH_MINUTES + 1), now)).toMatch(/^Free until/);
  });

  it("dates a clash from the later start and skips finished meetings", () => {
    expect(dayShape([
      meeting("Done", at(8), at(9)),
      meeting("Long", at(10, 30), at(12)),
      meeting("Overlap", at(11), at(11, 30)),
    ], now)).toBe(`${clock(11)} double-booked · Last meeting ends ${clock(12)}`);
  });

  it("says when no meeting is left, and says nothing for a day without meetings", () => {
    expect(dayShape([meeting("Done", at(8), at(9))], now)).toBe("No more meetings today");
    expect(dayShape([], now)).toBe("");
  });
});

describe("meetingsWith", () => {
  it("finds the meetings still ahead that a sender is invited to", () => {
    const events = [
      meeting("Earlier", at(8), at(9), { attendees: ["taylor@example.com"] }),
      meeting("Ops", at(15), at(16), { attendees: ["taylor@example.com"] }),
      meeting("Marketing", at(14), at(14, 25), { attendees: ["taylor@example.com", "nick@example.com"] }),
      meeting("Skipped", at(17), at(18), { attendees: ["taylor@example.com"], responseStatus: "declined" }),
      meeting("Other", at(13), at(14), { attendees: ["nick@example.com"] }),
    ];
    const shared = meetingsWith("Taylor@Example.com", events, now);
    expect(shared.map((event) => event.id)).toEqual(["Marketing", "Ops"]);
    expect(sharedMeetingsNote(shared)).toBe(`Also in your ${clock(14)} and ${clock(15)} meetings`);
    expect(sharedMeetingsNote(shared.slice(0, 1))).toBe(`Also in your ${clock(14)} meeting`);
    expect(sharedMeetingsNote([])).toBe("");
  });
});
