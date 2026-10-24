import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { CalendarOption, ContactProfile, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { nextMeeting, TodayWorkspace } from "./TodayWorkspace";
import { todayItem } from "./test/todayMail";

const openUrl = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
  return {
    id, accountId: "you@example.com", threadId: null, sourceMessageId: null, subjectSnapshot: null,
    title: `Task ${id}`, notes: null, kind: "action", dueKind: "none", dueValue: null, timeZone: null,
    repeatIntervalDays: null, status: "open", completionSource: null, evidenceText: null, waitAfter: null,
    createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null, ...overrides,
  };
}

function contact(id: string, name: string | null, dueAt: string | null): ContactProfile {
  return {
    id, displayName: name, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null,
    favorite: false, addresses: [`${id}@example.com`], sentCount: 0, receivedCount: 0, lastInteractedAt: null, birthday: null,
    keepInTouch: { intervalDays: 30, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null },
    keepInTouchDueAt: dueAt,
  };
}

function event(id: string, startOffsetMinutes: number, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent {
  const start = new Date(Date.now() + startOffsetMinutes * 60_000);
  return {
    id, accountId: "cal@example.com", calendarId: "primary", title: `Event ${id}`, allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString(), ...overrides,
  };
}

const calendars: CalendarOption[] = [{ id: "primary", accountId: "cal@example.com", name: "Work", primary: true, selected: true, writable: true }];
const daysFromNow = (days: number) => new Date(Date.now() + days * 86_400_000).toISOString();
const localDate = (days: number) => {
  const date = new Date(Date.now() + days * 86_400_000);
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
};

function renderToday(overrides: Partial<Parameters<typeof TodayWorkspace>[0]> = {}) {
  const handlers = {
    onOpenThread: vi.fn(), onOpenEvent: vi.fn(), onEditTask: vi.fn(), onTasksChanged: vi.fn(), onOpenInbox: vi.fn(),
    onOpenTasks: vi.fn(), onOpenContact: vi.fn(), onOpenKeepInTouch: vi.fn(),
  };
  render(<TodayWorkspace accountId="you@example.com" refreshKey="" taskRefreshKey={0} calendarConnected={false} calendars={calendars} {...handlers} {...overrides} />);
  return handlers;
}

function mockMail(threads: TodayMail[], total = threads.length) {
  return vi.spyOn(mailClient, "listTodayMail").mockResolvedValue({ threads, total });
}

describe("TodayWorkspace", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    mockMail([]);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); openUrl.mockReset(); localStorage.clear(); });

  describe("mail", () => {
    it("groups unread mail for the account in view and opens a conversation", async () => {
      const listTodayMail = mockMail([
        todayItem("ask", { sender: "Dana Ortiz <dana@example.org>" }, { subject: "Q4 budget" }),
        todayItem("hello", { sender: "Priya <priya@example.org>", corresponded: false }),
        todayItem("news", { listUnsubscribe: true }),
      ]);
      const { onOpenThread } = renderToday();

      const reply = await screen.findByRole("region", { name: "Needs a reply" });
      expect(listTodayMail).toHaveBeenCalledWith("you@example.com");
      expect(within(reply).getByText("Dana Ortiz")).toBeInTheDocument();
      expect(within(reply).getByText("Q4 budget")).toBeInTheDocument();
      expect(within(screen.getByRole("region", { name: "From people" })).getByText("Priya")).toBeInTheDocument();
      expect(within(screen.getByRole("region", { name: "Updates" })).getAllByRole("button", { name: /Person news/ })).toHaveLength(1);

      fireEvent.click(within(reply).getByRole("button", { name: /Dana Ortiz/ }));
      expect(onOpenThread).toHaveBeenCalledWith("ask");
    });

    it("merges every account when none is selected", async () => {
      const listTodayMail = mockMail([]);
      renderToday({ accountId: null });
      await waitFor(() => expect(listTodayMail).toHaveBeenCalledWith(undefined));
      expect(screen.getByText("Today · Mail and tasks: All accounts")).toBeInTheDocument();
    });

    it("says when nothing is unread, and when only the newest are listed", async () => {
      renderToday();
      // The empty state keeps its heading, so the column lines up with the others.
      expect(within(await screen.findByRole("region", { name: "Unread mail" })).getByText("Nothing unread.")).toBeInTheDocument();
      cleanup();

      mockMail([todayItem("one")], 250);
      renderToday();
      expect(await screen.findByText("Showing the newest 1 of 250 unread.")).toBeInTheDocument();
      expect(screen.getByText(/250 unread/, { selector: ".today-summary" })).toBeInTheDocument();
    });

    it("shows a read failure instead of an empty inbox", async () => {
      vi.spyOn(mailClient, "listTodayMail").mockRejectedValue(new Error("Database is locked"));
      renderToday();
      expect(within(await screen.findByRole("region", { name: "Unread mail" })).getByRole("alert")).toHaveTextContent("Database is locked");
      expect(screen.queryByText("Nothing unread.")).not.toBeInTheDocument();
    });

    it("opens the Inbox from the mail column", async () => {
      const { onOpenInbox } = renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Open Inbox" }));
      expect(onOpenInbox).toHaveBeenCalled();
    });
  });

  describe("tasks", () => {
    it("splits tasks into overdue, due today, and the rest soonest first, and opens them", async () => {
      const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([
        task("later", { dueKind: "date", dueValue: "2099-01-01" }),
        task("done", { status: "completed", completedAt: "2026-10-01T00:00:00Z" }),
        task("soon", { dueKind: "date", dueValue: "2098-01-01" }),
        task("late", { dueKind: "date", dueValue: localDate(-2) }),
        task("now", { dueKind: "date", dueValue: localDate(0) }),
      ]);
      const { onEditTask, onOpenTasks } = renderToday();

      const open = await screen.findByRole("region", { name: "Open tasks" });
      expect(listTasks).toHaveBeenCalledWith("you@example.com");
      const titles = within(open).getAllByRole("button", { name: /^Task / }).map((button) => button.textContent);
      expect(titles[0]).toContain("Task soon");
      expect(titles[1]).toContain("Task later");
      expect(screen.queryByText("Task done")).not.toBeInTheDocument();
      expect(within(screen.getByRole("region", { name: "Overdue" })).getByText("Task late")).toBeInTheDocument();
      expect(within(screen.getByRole("region", { name: "Due today" })).getByText("Task now")).toBeInTheDocument();
      expect(screen.getByText(/2 tasks due/, { selector: ".today-summary" })).toBeInTheDocument();

      fireEvent.click(within(open).getByRole("button", { name: /Task soon/ }));
      expect(onEditTask).toHaveBeenCalledWith(expect.objectContaining({ id: "soon" }));
      fireEvent.click(screen.getByRole("button", { name: "Open Tasks" }));
      expect(onOpenTasks).toHaveBeenCalled();
    });

    it("checks a task off in place and reports the change", async () => {
      vi.spyOn(mailClient, "listTasks").mockResolvedValue([task("now", { dueKind: "date", dueValue: localDate(0) })]);
      const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue(task("now", { status: "completed", dueKind: "date", dueValue: localDate(0) }));
      const { onTasksChanged } = renderToday();

      fireEvent.click(await screen.findByRole("checkbox", { name: "Mark Task now done" }));
      await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("now", "completed"));
      expect(await screen.findByRole("checkbox", { name: "Mark Task now not done" })).toBeChecked();
      expect(onTasksChanged).toHaveBeenCalled();
    });

    it("lists only people who are due or overdue, and opens their contact", async () => {
      vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([
        contact("future", "Future Friend", daysFromNow(20)),
        contact("overdue", "Ada Lovelace", daysFromNow(-3)),
        contact("off", "No Reminder", null),
      ]);
      const { onOpenContact, onOpenKeepInTouch } = renderToday();

      const section = await screen.findByRole("region", { name: "Keep in touch" });
      expect(within(section).getByText("Ada Lovelace")).toBeInTheDocument();
      expect(within(section).getByText(/^Overdue since /)).toBeInTheDocument();
      expect(within(section).queryByText("Future Friend")).not.toBeInTheDocument();
      expect(within(section).queryByText("No Reminder")).not.toBeInTheDocument();

      fireEvent.click(within(section).getByRole("button", { name: /Ada Lovelace/ }));
      expect(onOpenContact).toHaveBeenCalledWith("overdue");
      fireEvent.click(within(section).getByRole("button", { name: "Open Keep in Touch" }));
      expect(onOpenKeepInTouch).toHaveBeenCalled();
    });

    it("says there are no open tasks only once they have been read", async () => {
      let resolve: (tasks: ThreadTask[]) => void = () => {};
      vi.spyOn(mailClient, "listTasks").mockReturnValue(new Promise((done) => { resolve = done; }));
      renderToday();
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
      expect(screen.queryByText("No open tasks.")).not.toBeInTheDocument();
      expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();

      resolve([]);
      expect(within(await screen.findByRole("region", { name: "Open tasks" })).getByText("No open tasks.")).toBeInTheDocument();
    });

    it("leaves sections out when their data cannot be read", async () => {
      vi.spyOn(mailClient, "listTasks").mockRejectedValue(new Error("locked"));
      vi.spyOn(mailClient, "listKeepInTouch").mockRejectedValue(new Error("locked"));
      renderToday();

      await waitFor(() => expect(mailClient.listKeepInTouch).toHaveBeenCalled());
      expect(screen.queryByRole("region", { name: "Keep in touch" })).not.toBeInTheDocument();
      expect(screen.queryByRole("region", { name: "Open tasks" })).not.toBeInTheDocument();
    });
  });

  describe("agenda", () => {
    it("points to Settings instead of loading a schedule when no calendar is connected", async () => {
      renderToday({ calendarConnected: false });
      expect(within(screen.getByRole("region", { name: "Today's agenda" })).getByText(/Connect a calendar in Settings/)).toBeInTheDocument();
      expect(screen.getByText("Today · Mail and tasks: you@example.com")).toBeInTheDocument();
      expect(mailClient.listScheduleEvents).not.toHaveBeenCalled();
      expect(await screen.findByText(/^0 unread · 0 tasks due$/, { selector: ".today-summary" })).toBeInTheDocument();
    });

    it("says the agenda covers every calendar while mail and tasks follow the account", async () => {
      renderToday({ calendarConnected: true });
      expect(await screen.findByText("Today · Mail and tasks: you@example.com · All calendars")).toBeInTheDocument();
    });

    it("counts a meeting on two calendars once, and skips declined ones for the count and the next meeting", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("work", 30, { title: "Sync", accountId: "cal@example.com" }),
        event("copy", 30, { title: "Sync", accountId: "cal@example.com", responseStatus: "declined" }),
        event("skip", 10, { title: "Optional talk", responseStatus: "declined" }),
      ] });
      renderToday({ calendarConnected: true });

      expect(await screen.findByRole("button", { name: /^Next: Sync in \d+ min$/ })).toBeInTheDocument();
      expect(screen.getByText(/^1 meeting ·/, { selector: ".today-summary" })).toBeInTheDocument();
      const agenda = screen.getByRole("region", { name: "Today's agenda" });
      expect(within(agenda).getAllByText("Sync")).toHaveLength(1);
      expect(within(agenda).getByText("Optional talk").closest(".context-row")).toHaveClass("agenda-event-declined");
    });

    it("names a meeting under way as now, with when it ends", async () => {
      const ongoing = event("ongoing", -10, { title: "Standup" });
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [ongoing] });
      renderToday({ calendarConnected: true });
      expect(await screen.findByRole("button", { name: /^Now: Standup · until \d{1,2}:\d{2}/ })).toBeInTheDocument();
    });

    it("shows today's agenda, counts meetings, and offers to join the next one", async () => {
      // Late in the day the fixtures could cross midnight, so they stay close to now.
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("past", -60),
        event("next", 2, { title: "Design sync", conferenceUrl: "https://meet.example.com/design" }),
      ] });
      const { onOpenEvent } = renderToday({ calendarConnected: true });

      const agenda = await screen.findByRole("region", { name: "Today's agenda" });
      await within(agenda).findByText("Design sync");
      const next = await screen.findByRole("button", { name: /^Next: Design sync in \d+ min$/ });
      expect(screen.getByText(/2 meetings/, { selector: ".today-summary" })).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Join" }));
      expect(openUrl).toHaveBeenCalledWith("https://meet.example.com/design");
      fireEvent.click(next);
      expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "next" }));
    });
  });
});

describe("TodayWorkspace tomorrow", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    vi.spyOn(mailClient, "listTodayMail").mockResolvedValue({ threads: [], total: 0 });
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("loads through tomorrow and lists tomorrow under today's agenda, apart from today's count", async () => {
    const tomorrowNoon = new Date();
    tomorrowNoon.setDate(tomorrowNoon.getDate() + 1);
    tomorrowNoon.setHours(12, 0, 0, 0);
    const listEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
      event("dentist", (tomorrowNoon.getTime() - Date.now()) / 60_000, { title: "Dentist" }),
    ] });
    render(<TodayWorkspace accountId={null} refreshKey="" taskRefreshKey={0} calendarConnected calendars={calendars}
      onOpenThread={vi.fn()} onOpenEvent={vi.fn()} onEditTask={vi.fn()} onTasksChanged={vi.fn()} onOpenInbox={vi.fn()}
      onOpenTasks={vi.fn()} onOpenContact={vi.fn()} onOpenKeepInTouch={vi.fn()} />);

    const tomorrow = await screen.findByRole("region", { name: "Tomorrow" });
    expect(await within(tomorrow).findByText("Dentist")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Today's agenda" })).queryByText("Dentist")).not.toBeInTheDocument();
    expect(screen.getByText(/0 meetings/, { selector: ".today-summary" })).toBeInTheDocument();
    const [timeMin, timeMax] = listEvents.mock.calls[0];
    expect(new Date(timeMax).getTime() - new Date(timeMin).getTime()).toBeGreaterThanOrEqual(47 * 3_600_000);
  });
});

describe("nextMeeting", () => {
  const now = new Date("2026-10-23T12:00:00Z");
  const at = (id: string, start: string, end: string, allDay = false): ScheduleEvent =>
    ({ id, accountId: "a", title: id, start, end, allDay });

  it("prefers a meeting under way, then the soonest to start, and skips all-day and finished events", () => {
    expect(nextMeeting([
      at("later", "2026-10-23T15:00:00Z", "2026-10-23T16:00:00Z"),
      at("allDay", "2026-10-23", "2026-10-24", true),
      at("done", "2026-10-23T09:00:00Z", "2026-10-23T10:00:00Z"),
      at("now", "2026-10-23T11:30:00Z", "2026-10-23T12:30:00Z"),
    ], now)?.id).toBe("now");
    expect(nextMeeting([at("done", "2026-10-23T09:00:00Z", "2026-10-23T10:00:00Z")], now)).toBeNull();
  });
});
