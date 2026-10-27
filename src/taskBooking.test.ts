import { describe, expect, it } from "vitest";
import type { CalendarAccount, CalendarOption, ScheduleEvent, ThreadTask } from "./domain";
import { bookedBlock, defaultEventCalendar, openTasksFor, TASK_BLOCK_MINUTES, taskBlockRequest, writableCalendars } from "./taskBooking";

function calendar(id: string, accountId: string, overrides: Partial<CalendarOption> = {}): CalendarOption {
  return { id, accountId, name: id, primary: false, selected: true, writable: true, ...overrides };
}

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "joel@example.com", threadId: null, subjectSnapshot: null, title: id, kind: "action",
    dueKind: "none", status: "open", createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z", ...overrides,
  };
}

function block(id: string, start: string, end: string, sourceTaskId: string | null = "proposal"): ScheduleEvent {
  return { id, accountId: "joel@example.com", title: id, start, end, allDay: false, sourceTaskId };
}

describe("task booking", () => {
  it("only offers writable calendars on connected accounts", () => {
    const accounts: CalendarAccount[] = [
      { email: "joel@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "connected" },
      { email: "old@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "needs_reauth" },
    ];
    const calendars = [calendar("primary", "joel@example.com"), calendar("holidays", "joel@example.com", { writable: false }), calendar("old", "old@example.com")];
    expect(writableCalendars(accounts, calendars).map((option) => option.id)).toEqual(["primary"]);
  });

  it("prefers the task's account, then a visible primary calendar, falling back to any writable one", () => {
    const writable = [
      calendar("team", "joel@example.com"),
      calendar("joel", "joel@example.com", { primary: true }),
      calendar("work-team", "work@example.com", { selected: false }),
      calendar("work", "work@example.com", { primary: true, selected: false }),
    ];
    expect(defaultEventCalendar(writable)?.id).toBe("joel");
    expect(defaultEventCalendar(writable, "work@example.com")?.id).toBe("work");
    expect(defaultEventCalendar([writable[2]!, writable[0]!], "work@example.com")?.id).toBe("work-team");
    // An account without a writable calendar books on the usual default.
    expect(defaultEventCalendar(writable, "imap:home")?.id).toBe("joel");
    expect(defaultEventCalendar([])).toBeUndefined();
  });

  it("keeps To Do and In Progress tasks of the shown accounts, soonest due first", () => {
    const tasks = [
      task("undated"),
      task("later", { dueKind: "date", dueValue: "2026-10-30" }),
      task("doing", { status: "in_progress", dueKind: "date", dueValue: "2026-10-01" }),
      task("done", { status: "completed" }),
      task("cancelled", { status: "cancelled" }),
      task("elsewhere", { accountId: "work@example.com" }),
    ];
    expect(openTasksFor(tasks, ["joel@example.com"]).map((item) => item.id)).toEqual(["doing", "later", "undated"]);
    expect(openTasksFor(tasks, [])).toEqual([]);
  });

  it("asks for a thirty-minute block tagged with the task", () => {
    const start = new Date(2026, 9, 27, 14, 0);
    expect(TASK_BLOCK_MINUTES).toBe(30);
    expect(taskBlockRequest(task("proposal", { title: " Write proposal " }), calendar("primary", "joel@example.com"), start)).toEqual({
      accountId: "joel@example.com", calendarId: "primary", title: "Write proposal",
      start: start.toISOString(), end: new Date(2026, 9, 27, 14, 30).toISOString(),
      description: "", attendees: [], sourceTaskId: "proposal",
    });
  });

  it("names the next booked block still to end, else the latest one", () => {
    const proposal = task("proposal");
    const now = new Date("2026-10-27T12:00:00Z");
    const past = block("past", "2026-10-26T09:00:00Z", "2026-10-26T09:30:00Z");
    const next = block("next", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z");
    const after = block("after", "2026-10-29T09:00:00Z", "2026-10-29T09:30:00Z");
    const other = block("other", "2026-10-27T13:00:00Z", "2026-10-27T13:30:00Z", "budget");
    expect(bookedBlock(proposal, [after, other, past, next], now)?.id).toBe("next");
    expect(bookedBlock(proposal, [past, other], now)?.id).toBe("past");
    expect(bookedBlock(proposal, [other, block("untagged", "2026-10-28T09:00:00Z", "2026-10-28T09:30:00Z", null)], now)).toBeNull();
  });
});
