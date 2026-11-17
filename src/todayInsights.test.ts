import { describe, expect, it } from "vitest";
import { formatClockTime } from "./agendaEvents";
import type { ScheduleEvent } from "./domain";
import { agendaConflicts, dayShape, daySummary, EVENING_HINT_MINUTES, FREE_STRETCH_MINUTES, meetingsWith, mentionsRecording, eveningOpens, EVENING_FALLBACK_HOUR, offersPlanTomorrow, planCapacity, sharedMeetingsNote, type WorkingHours } from "./todayInsights";
import { TASK_BLOCK_MINUTES } from "./taskBooking";

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
    ], now)).toBe(`Free until ${clock(14)} · Double-booked at ${clock(16)} (CS Updates is recorded) · Last meeting ends ${clock(17)}`);
  });

  it("counts two or more clashes rather than listing each time", () => {
    expect(dayShape([
      meeting("Marketing", at(14), at(14, 25)),
      meeting("Momentum", at(14), at(14, 45)),
      meeting("CS Updates", at(16), at(16, 20), { description: "The session is recorded." }),
      meeting("Ramp AI", at(16), at(17)),
    ], now)).toBe(`Free until ${clock(14)} · 2 double-bookings (CS Updates is recorded) · Last meeting ends ${clock(17)}`);
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
    ], now)).toBe(`Double-booked at ${clock(11)} · Last meeting ends ${clock(12)}`);
  });

  it("says when no meeting is left, and says nothing for a day without meetings", () => {
    expect(dayShape([meeting("Done", at(8), at(9))], now)).toBe("No more meetings today");
    expect(dayShape([], now)).toBe("");
  });
});

describe("daySummary", () => {
  const benefits = meeting("Benefits Review", at(11), at(12));
  const ops = meeting("Operations", at(15), at(15, 30));

  it("says there are no meetings, counting neither declined nor all-day events", () => {
    expect(daySummary([], now)).toBe("No meetings today");
    expect(daySummary([meeting("Skip", at(11), at(12), { responseStatus: "declined" }), meeting("Offsite", at(0), at(23, 59), { allDay: true })], now)).toBe("No meetings today");
  });

  it("leaves out the next or current meeting on request, and never counts a working location as a meeting", () => {
    expect(daySummary([benefits, ops], at(9), { omitNext: true })).toBe(`2 meetings · last meeting ends ${clock(15, 30)}`);
    expect(daySummary([benefits, ops], at(11, 10), { omitNext: true })).toBe(`2 meetings · last meeting ends ${clock(15, 30)}`);
    expect(daySummary([benefits, ops], at(16), { omitNext: true })).toBe(daySummary([benefits, ops], at(16)));
    const office = meeting("Office", at(9), at(17), { eventType: "workingLocation" });
    expect(daySummary([office, ops], at(9))).toBe(`1 meeting · next at ${clock(15)}`);
    expect(daySummary([office], at(9))).toBe("No meetings today");
    expect(dayShape([office], at(9))).toBe("");
  });

  it("names the next meeting in the morning, and when the last ends while more than one is left", () => {
    expect(daySummary([benefits, ops], at(9))).toBe(`2 meetings · next at ${clock(11)} · last meeting ends ${clock(15, 30)}`);
    expect(daySummary([ops], at(9))).toBe(`1 meeting · next at ${clock(15)}`);
  });

  it("names the meeting under way, and the next between meetings", () => {
    expect(daySummary([benefits, ops], at(11, 15))).toBe(`2 meetings · in Benefits Review until ${clock(12)} · last meeting ends ${clock(15, 30)}`);
    expect(daySummary([benefits, ops], at(13))).toBe(`2 meetings · next at ${clock(15)}`);
  });

  it("says when every meeting has finished", () => {
    expect(daySummary([benefits, ops], at(17, 30))).toBe(`2 meetings, both finished · last ended ${clock(15, 30)}`);
    expect(daySummary([benefits], at(17, 30))).toBe(`1 meeting, finished · ended ${clock(12)}`);
    expect(daySummary([benefits, ops, meeting("Late", at(16), at(16, 30))], at(17, 30))).toBe(`3 meetings, all finished · last ended ${clock(16, 30)}`);
  });

  it("keeps double-bookings still ahead", () => {
    const clash = meeting("Clash", at(15, 15), at(16));
    expect(daySummary([benefits, ops, clash], at(13))).toBe(`3 meetings · next at ${clock(15)} · double-booked at ${clock(15, 15)} · last meeting ends ${clock(16)}`);
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

describe("planCapacity", () => {
  // Monday 16 November 2026, in UTC so the windows read as written.
  const hours: WorkingHours = { timeZone: "UTC", workingWindows: [{ weekday: 1, start: "09:00", end: "12:00" }, { weekday: 1, start: "13:00", end: "17:00" }] };
  const utc = (clock: string) => new Date(`2026-11-16T${clock}:00Z`);
  const busy = (id: string, start: string, end: string, overrides: Partial<ScheduleEvent> = {}) => meeting(id, utc(start), utc(end), overrides);
  const minutes = (capacity: ReturnType<typeof planCapacity>) => capacity?.stretches.map((stretch) => stretch.minutes);

  it("is unknown without a schedule or working hours, never an open day", () => {
    expect(planCapacity(null, hours, utc("08:00"))).toBeNull();
    expect(planCapacity([], null, utc("08:00"))).toBeNull();
  });

  it("counts working windows from their start, or from now, to their end", () => {
    const early = planCapacity([], hours, utc("08:00"));
    expect(early).toMatchObject({ day: "2026-11-16", workingMinutes: 420, freeMinutes: 420, bookedBlocks: 0 });
    expect(minutes(early)).toEqual([180, 240]);
    expect(minutes(planCapacity([], hours, utc("10:15")))).toEqual([105, 240]);
    expect(minutes(planCapacity([], hours, utc("12:30")))).toEqual([240]);
    expect(planCapacity([], hours, utc("16:59"))?.freeMinutes).toBe(1);
    expect(planCapacity([], hours, utc("17:00"))?.stretches).toEqual([]);
    expect(planCapacity([], hours, utc("18:00"))?.freeMinutes).toBe(0);
  });

  it("finds nothing free outside working hours or on a day without them", () => {
    const sunday = planCapacity([], hours, new Date("2026-11-15T10:00:00Z"));
    expect(sunday).toMatchObject({ day: "2026-11-15", workingMinutes: 0, freeMinutes: 0, stretches: [] });
    // A later day is counted from its start, whatever the time now.
    expect(planCapacity([], hours, utc("16:00"), "2026-11-23")?.freeMinutes).toBe(420);
  });

  it("merges overlapping windows and splits free time around attending meetings and booked blocks", () => {
    const overlapping: WorkingHours = { timeZone: "UTC", workingWindows: [{ weekday: 1, start: "09:00", end: "11:00" }, { weekday: 1, start: "10:00", end: "12:00" }] };
    expect(planCapacity([], overlapping, utc("08:00"))?.workingMinutes).toBe(180);
    const events = [
      busy("Straddles the start", "08:30", "09:30"),
      busy("Booked", "10:00", "10:30", { sourceTaskId: "task-1" }),
      busy("Declined", "13:00", "15:00", { responseStatus: "declined" }),
      { ...busy("Holiday", "00:00", "23:59"), allDay: true, start: "2026-11-16", end: "2026-11-17" },
      busy("Office", "13:00", "17:00", { eventType: "workingLocation" }),
      busy("After hours", "18:00", "19:00"),
      busy("Overlaps lunch", "11:45", "13:15"),
    ];
    const capacity = planCapacity(events, hours, utc("08:00"));
    expect(minutes(capacity)).toEqual([30, 75, 225]);
    expect(capacity?.stretches[0].start).toEqual(utc("09:30"));
    expect(capacity?.bookedBlocks).toBe(1);
  });

  it("marks stretches that hold a task block and ones long enough to call free time", () => {
    const shape = (gap: number) => {
      const end = new Date(utc("09:00").getTime() + gap * 60_000);
      const stretch = planCapacity([busy("Next", end.toISOString().slice(11, 16), "12:00")], { timeZone: "UTC", workingWindows: [{ weekday: 1, start: "09:00", end: "12:00" }] }, utc("08:00"))!.stretches[0];
      return [stretch.minutes, stretch.holdsBlock, stretch.named];
    };
    expect(shape(TASK_BLOCK_MINUTES - 1)).toEqual([29, false, false]);
    expect(shape(TASK_BLOCK_MINUTES)).toEqual([30, true, false]);
    expect(shape(TASK_BLOCK_MINUTES + 1)).toEqual([31, true, false]);
    expect(shape(FREE_STRETCH_MINUTES - 1)).toEqual([59, true, false]);
    expect(shape(FREE_STRETCH_MINUTES)).toEqual([60, true, true]);
    expect(shape(FREE_STRETCH_MINUTES + 1)).toEqual([61, true, true]);
  });

  it("reads working hours in their own zone", () => {
    const newYork: WorkingHours = { timeZone: "America/New_York", workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] };
    const capacity = planCapacity([], newYork, new Date("2026-11-16T12:00:00Z"));
    expect(capacity?.stretches[0].start).toEqual(new Date("2026-11-16T14:00:00Z"));
    expect(capacity?.freeMinutes).toBe(480);
  });
});

describe("offersPlanTomorrow", () => {
  // Monday 16 November 2026, in UTC: working hours end at 17:00.
  const hours: WorkingHours = { timeZone: "UTC", workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] };
  const utc = (clock: string) => new Date(`2026-11-16T${clock}:00Z`);

  it("offers Plan Tomorrow from the last hour of working hours on, not before", () => {
    expect(EVENING_HINT_MINUTES).toBe(60);
    expect(offersPlanTomorrow([], hours, utc("15:59"))).toBe(false);
    expect(offersPlanTomorrow([], hours, utc("16:00"))).toBe(true);
    expect(offersPlanTomorrow([], hours, utc("18:30"))).toBe(true);
    expect(offersPlanTomorrow([], null, utc("18:30"))).toBe(false);
    // A day without working hours has no evening of its own.
    expect(offersPlanTomorrow([], { ...hours, workingWindows: [] }, utc("18:30"))).toBe(false);
  });

  it("offers it once today's last attended meeting has ended, ignoring declined and all-day ones", () => {
    const sync = meeting("sync", utc("10:00"), utc("11:00"));
    const later = meeting("later", utc("14:00"), utc("15:00"), { responseStatus: "declined" });
    const allDay = meeting("offsite", utc("00:00"), utc("23:59"), { allDay: true });
    expect(offersPlanTomorrow([sync], hours, utc("10:59"))).toBe(false);
    expect(offersPlanTomorrow([sync], hours, utc("11:00"))).toBe(true);
    expect(offersPlanTomorrow([sync, later, allDay], hours, utc("11:30"))).toBe(true);
    expect(offersPlanTomorrow([later], hours, utc("15:30"))).toBe(false);
  });
});

describe("eveningOpens", () => {
  const hours: WorkingHours = { timeZone: "UTC", workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] };
  const utc = (clock: string) => new Date(`2026-11-16T${clock}:00Z`);

  it("opens once the last attending timed meeting has ended and working hours are over, not before either", () => {
    const sync = meeting("sync", utc("10:00"), utc("11:00"));
    const late = meeting("late", utc("16:00"), utc("17:30"));
    expect(eveningOpens([sync], hours, utc("16:59"))).toBe(false);
    expect(eveningOpens([sync], hours, utc("17:00"))).toBe(true);
    // A meeting that runs past the end of working hours keeps the day view until it ends.
    expect(eveningOpens([sync, late], hours, utc("17:15"))).toBe(false);
    expect(eveningOpens([sync, late], hours, utc("17:30"))).toBe(true);
    // Hours over but a meeting still ahead stays in the day.
    expect(eveningOpens([meeting("dinner", utc("18:00"), utc("19:00"))], hours, utc("17:30"))).toBe(false);
  });

  it("ignores declined, all-day, and working-location events, and counts a day without meetings", () => {
    const declined = meeting("declined", utc("18:00"), utc("19:00"), { responseStatus: "declined" });
    const allDay = meeting("offsite", utc("00:00"), utc("23:59"), { allDay: true });
    const location = meeting("Office", utc("09:00"), utc("19:00"), { eventType: "workingLocation" });
    expect(eveningOpens([declined, allDay, location], hours, utc("17:30"))).toBe(true);
    expect(eveningOpens([], hours, utc("17:30"))).toBe(true);
  });

  it("never opens while the schedule is unknown", () => {
    expect(eveningOpens(null, hours, utc("20:00"))).toBe(false);
  });

  it("falls back to 6 pm local time without working hours, or on a day they don't cover", () => {
    expect(EVENING_FALLBACK_HOUR).toBe(18);
    const local = (hour: number, minute = 0) => new Date(2026, 10, 16, hour, minute);
    expect(eveningOpens([], null, local(17, 59))).toBe(false);
    expect(eveningOpens([], null, local(18))).toBe(true);
    // Working hours that cover no window today (a weekend) have no end to wait for.
    const noWindows: WorkingHours = { timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, workingWindows: [] };
    expect(eveningOpens([], noWindows, local(17, 59))).toBe(false);
    expect(eveningOpens([], noWindows, local(18, 30))).toBe(true);
  });
});
