import { planItemKey } from "./dailyPlan";
import { refreshSyncedData } from "./refreshSyncedData";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { PlanCandidate as SourceCandidate, PlanSourceKind, PlanSourcePage } from "./domain";
import type { CalendarAccount, CalendarOption, ContactProfile, DailyPlan, DailyPlanItem, Note, OpenPromise, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { HOUR_HEIGHT } from "./calendarTime";
import type { WorkingHours } from "./todayInsights";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import postcss from "postcss";
import { nextMeeting, TodayWorkspace } from "./TodayWorkspace";
import { runWorkspaceCommand } from "./workspaceCommands";
import { todayItem, todayThread } from "./test/todayMail";

vi.mock("./refreshSyncedData", () => ({ refreshSyncedData: vi.fn(async () => {}) }));

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
    onOpenThread: vi.fn(), onOpenEvent: vi.fn(), onEditTask: vi.fn(), onTasksChanged: vi.fn(), onOpenInbox: vi.fn(), onOpenCalendar: vi.fn(),
    onOpenTasks: vi.fn(), onOpenContact: vi.fn(), onOpenKeepInTouch: vi.fn(),
  };
  const page = (next: Partial<Parameters<typeof TodayWorkspace>[0]> = {}) => <TodayWorkspace accountId="you@example.com" refreshKey="" taskRefreshKey={0} calendarConnected={false} calendars={calendars} {...handlers} {...overrides} {...next} />;
  const view = render(page());
  return { ...handlers, rerenderToday: (next: Partial<Parameters<typeof TodayWorkspace>[0]>) => view.rerender(page(next)) };
}

function mockMail(threads: TodayMail[], total = threads.length) {
  return vi.spyOn(mailClient, "listTodayMail").mockResolvedValue({ threads, total });
}

const emptyPlan = (date = localDate(0)): DailyPlan => ({ date, noteId: `daily:${date}`, revision: null, items: [], notToday: [] });

function planItem(key: string, overrides: Partial<DailyPlanItem> = {}): DailyPlanItem {
  return {
    // Keyed as natively: the canonical target.
    key: planItemKey({ version: 1, kind: "task", id: key }), kind: "task", reference: { version: 1, kind: "task", id: key }, title: `Task ${key}`,
    state: "open", marker: null, carryCount: 0, taskId: key, accountId: "you@example.com", goal: null, contactId: null, start: 0, end: 0, ...overrides,
  };
}

function note(id: string, updatedAt = "v1"): Note {
  return { id, title: id, body: "", dailyDate: null, createdAt: updatedAt, updatedAt, trashed: false, trashedAt: null };
}

const calendarAccounts: CalendarAccount[] = [{ email: "cal@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];

/** Weekdays nine to five in this machine's zone, as the default availability profile has them. */
const workingHours: WorkingHours = { timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC", workingWindows: [1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })) };

/** A source tab, or a source behind More Sources. */
const chooseSource = (name: string) => {
  const tab = screen.queryByRole("tab", { name });
  if (tab) { fireEvent.click(tab); return; }
  fireEvent.click(screen.getByRole("button", { name: "More Sources" }));
  fireEvent.click(screen.getByRole("menuitem", { name }));
};

describe("TodayWorkspace", () => {
  beforeEach(() => {
    // Mid-morning, so the evening view (which opens by itself after hours) never depends on when the suite runs.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 10, 17, 10, 0));
    clearScheduleCache();
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([]);
    vi.spyOn(mailClient, "dailyPlan").mockImplementation(async (date) => emptyPlan(date));
    // Suggested is the first source; it reads planSources as soon as Today opens.
    vi.spyOn(mailClient, "planSources").mockResolvedValue({ next: null, candidates: [] });
    mockMail([]);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); openUrl.mockReset(); localStorage.clear(); });

  it.each(["you@example.com", null])("pulls fresh sources for the current account (%s) without changing agenda choices", async (accountId) => {
    let finish!: (status: Awaited<ReturnType<typeof mailClient.sync>>) => void;
    const status = await mailClient.syncStatus();
    const sync = vi.spyOn(mailClient, "sync").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    renderToday({ accountId, calendarConnected: true });
    await within(screen.getByRole("group", { name: "Mail" })).findByText("Nothing unread");
    fireEvent.click(screen.getByRole("button", { name: "Today's agenda" }));
    chooseSource("Tasks");
    const tasks = vi.mocked(mailClient.listTasks);
    tasks.mockResolvedValue([task("fresh", { title: "Fresh task" })]);
    fireEvent.wheel(screen.getByRole("region", { name: "Today" }), { deltaY: -100 });
    expect(await screen.findByText("Refreshing…")).toBeInTheDocument();
    await waitFor(() => expect(sync).toHaveBeenCalledWith(accountId ? { accountId, catchUp: false } : { catchUp: false }));
    await act(async () => { finish(status); });
    await screen.findByRole("button", { name: /^Fresh task/ });
    await waitFor(() => expect(screen.queryByText("Refreshing…")).not.toBeInTheDocument());
    expect(refreshSyncedData).toHaveBeenCalled();
    expect(tasks).toHaveBeenLastCalledWith(accountId ?? undefined);
    expect(screen.getByRole("button", { name: "Today's agenda" })).toHaveAttribute("aria-expanded", "false");
    expect(calendars[0].selected).toBe(true);
    expect(mailClient.listScheduleEvents).toHaveBeenCalledTimes(2);
    // The plan is read again with everything else.
    expect(vi.mocked(mailClient.dailyPlan).mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("does not restore the previous account after a slow pull finishes", async () => {
    let finish!: (status: Awaited<ReturnType<typeof mailClient.sync>>) => void;
    const status = await mailClient.syncStatus();
    const sync = vi.spyOn(mailClient, "sync").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    vi.mocked(mailClient.listTasks).mockImplementation(async (accountId) => [task(accountId!)]);
    const { rerenderToday } = renderToday();
    chooseSource("Tasks");
    await screen.findByRole("button", { name: "Task you@example.com" });
    fireEvent.wheel(screen.getByRole("region", { name: "Today" }), { deltaY: -100 });
    await waitFor(() => expect(sync).toHaveBeenCalled());
    rerenderToday({ accountId: "work@example.com" });
    await screen.findByRole("button", { name: "Task work@example.com" });
    await act(async () => { finish(status); });
    expect(screen.queryByRole("button", { name: "Task you@example.com" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Task work@example.com" })).toBeInTheDocument();
    expect(mailClient.listTodayMail).toHaveBeenLastCalledWith("work@example.com");
  });

  it("keeps a failed refresh's existing mail visible and reports the failure", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const sync = vi.spyOn(mailClient, "sync").mockRejectedValue(new Error("Server unavailable"));
    mockMail([todayItem("keep", { sender: "Dana <dana@example.org>", hasInvitation: true }, { subject: "Keep this conversation" })]);
    renderToday();
    await screen.findByText("Keep this conversation");
    fireEvent.wheel(screen.getByRole("region", { name: "Today" }), { deltaY: -100 });
    await screen.findByText("Refresh failed: Server unavailable");
    expect(sync).toHaveBeenCalled();
    expect(screen.getByText("Keep this conversation")).toBeInTheDocument();
  });

  describe("mail signal", () => {
    it("counts unread mail for the account in view, lists invitations, and opens them", async () => {
      const listTodayMail = mockMail([
        todayItem("ask", { sender: "Dana Ortiz <dana@example.org>" }, { subject: "Q4 budget" }),
        todayItem("hello", { sender: "Priya <priya@example.org>", corresponded: false }),
        todayItem("news", { listUnsubscribe: true }),
        todayItem("invite", { sender: "Meredith Reed <m@example.org>", hasInvitation: true }, { subject: "Invitation: Moving day" }),
      ]);
      const { onOpenThread, onOpenInbox } = renderToday();

      const signal = within(screen.getByRole("group", { name: "Mail" }));
      expect(await signal.findByText("1 needs a reply · 2 other unread")).toBeInTheDocument();
      expect(listTodayMail).toHaveBeenCalledWith("you@example.com");
      // An invitation is a calendar decision, so it is listed by name rather than counted.
      fireEvent.click(signal.getByRole("button", { name: "Invitation: Moving day" }));
      expect(onOpenThread).toHaveBeenCalledWith("invite");
      expect(signal.queryByText("Q4 budget")).not.toBeInTheDocument();
      fireEvent.click(signal.getByRole("button", { name: "Open Inbox" }));
      expect(onOpenInbox).toHaveBeenCalled();
      // The unread-mail column is gone.
      expect(screen.queryByRole("group", { name: "Unread mail" })).not.toBeInTheDocument();
    });

    it("collapses to one summary line, remembered on this device", async () => {
      mockMail([todayItem("ask"), todayItem("invite", { hasInvitation: true }, { subject: "Invitation: Picnic" })]);
      renderToday();
      const signal = within(screen.getByRole("group", { name: "Mail" }));
      await signal.findByRole("button", { name: "Invitation: Picnic" });
      fireEvent.click(signal.getByRole("button", { name: "Mail" }));
      expect(signal.getByRole("button", { name: "Mail" })).toHaveAttribute("aria-expanded", "false");
      expect(signal.getByText("1 invitation · 1 needs a reply")).toBeInTheDocument();
      expect(signal.queryByRole("button", { name: "Invitation: Picnic" })).not.toBeInTheDocument();
      cleanup();

      renderToday();
      const again = within(screen.getByRole("group", { name: "Mail" }));
      expect(again.getByRole("button", { name: "Mail" })).toHaveAttribute("aria-expanded", "false");
      expect(await again.findByText("1 invitation · 1 needs a reply")).toBeInTheDocument();
    });

    it("still works when this device's storage can't be used", async () => {
      vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
      vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
      renderToday();
      const signal = within(screen.getByRole("group", { name: "Mail" }));
      fireEvent.click(signal.getByRole("button", { name: "Mail" }));
      expect(signal.getByRole("button", { name: "Mail" })).toHaveAttribute("aria-expanded", "false");
      chooseSource("Tasks");
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveAttribute("aria-selected", "true");
    });

    it("counts each unread sender on their meetings still ahead in the day view", async () => {
      vi.useFakeTimers({ toFake: ["Date"] });
      vi.setSystemTime(new Date(2026, 9, 28, 10, 0));
      try {
        mockMail([
          todayItem("taylor", { sender: "Taylor <Taylor@example.org>" }, { subject: "Time off today" }),
          todayItem("dana", { sender: "Dana <dana@example.org>" }, { subject: "Q4 budget" }),
        ]);
        vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
          event("ops", 5, { title: "Operations", attendees: ["taylor@example.org"] }),
          event("gone", -60, { title: "Earlier", attendees: ["dana@example.org"] }),
        ] });
        renderToday({ calendarConnected: true });

        const day = within(screen.getByRole("group", { name: "Day view" }));
        expect(await day.findByRole("button", { name: /^Operations/ })).toHaveAccessibleDescription("1 unread from attendees");
        // A meeting that already ended is not counted.
        expect(day.getByRole("button", { name: /^Earlier/ })).not.toHaveAccessibleDescription(/unread/);
      } finally {
        vi.useRealTimers();
      }
    });

    it("merges every account when none is selected", async () => {
      const listTodayMail = mockMail([]);
      renderToday({ accountId: null });
      await waitFor(() => expect(listTodayMail).toHaveBeenCalledWith(undefined));
      expect(within(screen.getByRole("group", { name: "Mail" })).getByText("· All accounts", { selector: ".today-mail-signal-scope" })).toBeInTheDocument();
      expect(document.querySelector(".today-source-scope")).toHaveTextContent("All accounts");
    });

    it("says unread rather than other unread when nothing else is counted, and reads as a summary", async () => {
      mockMail([todayItem("news", { listUnsubscribe: true })]);
      renderToday();
      const signal = within(screen.getByRole("group", { name: "Mail" }));
      expect(await signal.findByText("1 unread")).toBeInTheDocument();
      // Only the chevron folds the line; the summary itself is text, not a link.
      expect(signal.getAllByRole("button").map((button) => button.getAttribute("aria-label") ?? button.textContent)).toEqual(["Mail", "Open Inbox"]);
      fireEvent.click(signal.getByRole("button", { name: "Mail" }));
      expect(signal.getByText("1 unread")).toBeInTheDocument();
    });

    it("says when nothing is unread, and counts unread past the newest it reads", async () => {
      renderToday();
      expect(await within(screen.getByRole("group", { name: "Mail" })).findByText("Nothing unread")).toBeInTheDocument();
      cleanup();

      mockMail([todayItem("one")], 250);
      renderToday();
      expect(await screen.findByText("1 needs a reply · 249 other unread")).toBeInTheDocument();
    });

    it("shows a read failure instead of an empty inbox", async () => {
      vi.spyOn(mailClient, "listTodayMail").mockRejectedValue(new Error("Database is locked"));
      renderToday();
      expect(await within(screen.getByRole("group", { name: "Mail" })).findByRole("alert")).toHaveTextContent("Database is locked");
      expect(screen.queryByText("Nothing unread")).not.toBeInTheDocument();
    });

    it("opens the Inbox, Calendar, and Tasks from Today, Tasks from its own tab", async () => {
      const { onOpenInbox, onOpenCalendar, onOpenTasks } = renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Open Inbox" }));
      expect(onOpenInbox).toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Open Calendar" }));
      expect(onOpenCalendar).toHaveBeenCalled();
      expect(screen.queryByRole("button", { name: "Open Tasks" })).not.toBeInTheDocument();
      chooseSource("Tasks");
      fireEvent.click(within(screen.getByRole("tabpanel")).getByRole("button", { name: "Open Tasks" }));
      expect(onOpenTasks).toHaveBeenCalled();
    });
  });

  describe("tasks", () => {
    it("shows overdue and due-today tasks under Also Due and every open task under Tasks, and opens them", async () => {
      const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue([
        task("later", { dueKind: "date", dueValue: "2099-01-01" }),
        task("done", { status: "completed", completedAt: "2026-10-01T00:00:00Z" }),
        task("soon", { dueKind: "date", dueValue: "2098-01-01" }),
        task("late", { dueKind: "date", dueValue: localDate(-2) }),
        task("now", { dueKind: "date", dueValue: localDate(0) }),
      ]);
      const { onEditTask, onOpenTasks } = renderToday();

      // Suggested comes first; with nothing suggested, Also Due lends its first cards beneath.
      expect(within(await screen.findByRole("region", { name: "Overdue" })).getByText("Task late")).toBeInTheDocument();
      expect(listTasks).toHaveBeenCalledWith("you@example.com");
      expect(within(screen.getByRole("region", { name: "Due today" })).getByText("Task now")).toBeInTheDocument();
      expect(screen.queryByText("Task soon")).not.toBeInTheDocument();
      expect(document.querySelector(".today-header")).not.toHaveTextContent(/due/);

      chooseSource("Tasks");
      const open = screen.getByRole("region", { name: "Open tasks" });
      const titles = within(open).getAllByRole("button", { name: /^Task / }).map((button) => button.textContent);
      expect(titles.findIndex((title) => title?.includes("Task soon"))).toBeLessThan(titles.findIndex((title) => title?.includes("Task later")));
      expect(screen.queryByText("Task done")).not.toBeInTheDocument();

      fireEvent.click(within(open).getByRole("button", { name: /^Task soon/ }));
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
      chooseSource("Keep in Touch");

      const section = await screen.findByRole("region", { name: "Keep in touch" });
      expect(within(section).getByText("Ada Lovelace")).toBeInTheDocument();
      expect(within(section).getByText(/^Overdue since /)).toBeInTheDocument();
      expect(within(section).queryByText("Future Friend")).not.toBeInTheDocument();
      expect(within(section).queryByText("No Reminder")).not.toBeInTheDocument();

      fireEvent.click(within(section).getByRole("button", { name: /^Ada Lovelace/ }));
      expect(onOpenContact).toHaveBeenCalledWith("overdue");
      fireEvent.click(within(section).getByRole("button", { name: "Open Keep in Touch" }));
      expect(onOpenKeepInTouch).toHaveBeenCalled();
    });

    it("says there are no open tasks only once they have been read", async () => {
      let resolve: (tasks: ThreadTask[]) => void = () => {};
      vi.spyOn(mailClient, "listTasks").mockReturnValue(new Promise((done) => { resolve = done; }));
      renderToday();
      chooseSource("Tasks");
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
      expect(screen.queryByText("No open tasks.")).not.toBeInTheDocument();

      resolve([]);
      expect(await within(screen.getByRole("tabpanel")).findByText("No open tasks.")).toBeInTheDocument();
    });

    it("leaves sections out when their data cannot be read", async () => {
      vi.spyOn(mailClient, "listTasks").mockRejectedValue(new Error("locked"));
      vi.spyOn(mailClient, "listKeepInTouch").mockRejectedValue(new Error("locked"));
      renderToday();

      await waitFor(() => expect(mailClient.listKeepInTouch).toHaveBeenCalled());
      chooseSource("Keep in Touch");
      expect(screen.queryByRole("region", { name: "Keep in touch" })).not.toBeInTheDocument();
      chooseSource("Tasks");
      expect(screen.queryByRole("region", { name: "Open tasks" })).not.toBeInTheDocument();
    });
  });

  describe("promises", () => {
    const promise = (id: string, overrides: Partial<OpenPromise> = {}): OpenPromise => ({
      thread: todayThread(`thread-${id}`, { subject: "ArtBrew report", accountId: "you@example.com" }),
      messageId: id, sentAt: "2026-10-22T10:00:00Z", recipients: ["Jill Hart <jill@example.org>"],
      excerpt: "I'll reconnect your server, run the report and send it over", ...overrides,
    });

    it("lists promises still waiting on you and opens the conversation", async () => {
      const listOpenPromises = vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([promise("p1")]);
      const { onOpenThread } = renderToday();
      chooseSource("You Said You Would");

      const section = await screen.findByRole("region", { name: "You said you would" });
      expect(listOpenPromises).toHaveBeenCalledWith("you@example.com");
      expect(within(section).getByText("To Jill Hart · Nothing sent to them since")).toBeInTheDocument();
      fireEvent.click(within(section).getByRole("button", { name: /^I.ll reconnect your server/ }));
      expect(onOpenThread).toHaveBeenCalledWith("thread-p1");
    });

    it("writes when a promise was sent the way event times are written", async () => {
      const today = new Date();
      today.setHours(11, 8, 0, 0);
      vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([promise("p1", { sentAt: today.toISOString() })]);
      renderToday();
      chooseSource("You Said You Would");
      const section = await screen.findByRole("region", { name: "You said you would" });
      expect(within(section).getByText(/^\d{1,2}:08/).textContent).not.toMatch(/[AP]M/);
    });

    it("makes an action task from a promise and opens it for editing", async () => {
      vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([promise("p1")]);
      const created = task("new", { title: "I'll reconnect your server, run the report and send it over", kind: "action" });
      const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue(created);
      const { onEditTask, onTasksChanged } = renderToday();
      chooseSource("You Said You Would");

      fireEvent.click(await screen.findByRole("button", { name: /^Make a task: I'll reconnect/ }));
      await waitFor(() => expect(createTask).toHaveBeenCalledWith({
        accountId: "you@example.com", threadId: "thread-p1", sourceMessageId: "p1", subjectSnapshot: "ArtBrew report",
        title: "I'll reconnect your server, run the report and send it over", kind: "action",
        evidenceText: "I'll reconnect your server, run the report and send it over",
      }));
      expect(onEditTask).toHaveBeenCalledWith(created);
      expect(onTasksChanged).toHaveBeenCalled();
      await waitFor(() => expect(screen.queryByRole("region", { name: "You said you would" })).not.toBeInTheDocument());
    });

    it("dismisses a promise on this device", async () => {
      vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([promise("p1"), promise("p2", { excerpt: "Let me check with Pete" })]);
      renderToday();
      chooseSource("You Said You Would");

      fireEvent.click(await screen.findByRole("button", { name: "Dismiss: Let me check with Pete" }));
      const section = screen.getByRole("region", { name: "You said you would" });
      expect(within(section).queryByText("Let me check with Pete")).not.toBeInTheDocument();
      cleanup();

      renderToday();
      // The chosen source is remembered too.
      expect(await screen.findByText(/^I'll reconnect your server/)).toBeInTheDocument();
      expect(screen.queryByText("Let me check with Pete")).not.toBeInTheDocument();
    });

    it("leaves the section out when promises cannot be read", async () => {
      vi.spyOn(mailClient, "listOpenPromises").mockRejectedValue(new Error("locked"));
      renderToday();
      chooseSource("You Said You Would");
      await waitFor(() => expect(mailClient.listOpenPromises).toHaveBeenCalled());
      expect(screen.queryByRole("region", { name: "You said you would" })).not.toBeInTheDocument();
    });
  });

  describe("plan", () => {
    beforeEach(() => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(2026, 9, 28, 10, 0)); });
    afterEach(() => { vi.useRealTimers(); });

    const at = (hours: number, minutes = 0) => new Date(2026, 9, 28, hours, minutes).toISOString();
    const items = () => [
      planItem("done", { title: "Send the deck", state: "done" }),
      planItem("focus", { title: "Review the contract", goal: { id: "g1", title: "Close Q4 deals" } }),
      { ...planItem("memo", { title: "Draft the memo", kind: "noteTask", taskId: "note:note-7:m1", accountId: null }), key: "memo" },
      { ...planItem("sam", { title: "Sam Ortiz", kind: "contact", taskId: null, accountId: null, contactId: "c-sam" }), key: "sam" },
    ];
    const withPlan = (planItems: DailyPlanItem[] = items()) => vi.mocked(mailClient.dailyPlan).mockImplementation(async (date) => ({ ...emptyPlan(date), revision: "r1", items: planItems }));

    it("says how often an item was carried, without reordering it", async () => {
      withPlan([
        planItem("a", { title: "First", carryCount: 3 }),
        planItem("b", { title: "Second", carryCount: 1 }),
        planItem("c", { title: "Third" }),
      ]);
      renderToday();
      const plan = within(await screen.findByRole("region", { name: "Today's plan" }));
      const cards = await plan.findAllByRole("article");
      expect(within(cards[0]).getByText("you@example.com · Carried 3 times")).toBeInTheDocument();
      expect(within(cards[1]).getByText("you@example.com · Carried 1 time")).toBeInTheDocument();
      expect(within(cards[2]).getByText("you@example.com")).toBeInTheDocument();
      expect(cards[0]).toHaveClass("today-plan-focus");
    });

    it("highlights a card and its booked block together on hover and focus", async () => {
      withPlan();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("block", 0, { title: "Review the contract", start: at(14), end: at(14, 30), sourceTaskId: "focus" }),
        event("memo-block", 0, { title: "Draft the memo", start: at(15), end: at(15, 30), sourceTaskId: "note:note-7:m1" }),
      ] });
      renderToday({ calendarConnected: true });
      const day = within(screen.getByRole("group", { name: "Day view" }));
      const block = await day.findByRole("button", { name: /^Review the contract/ });
      const card = (await within(screen.getByRole("region", { name: "Today's plan" })).findByText("Review the contract")).closest("article")!;
      fireEvent.mouseEnter(card);
      expect(block).toHaveAttribute("data-linked", "true");
      fireEvent.mouseLeave(card);
      expect(block).not.toHaveAttribute("data-linked");
      fireEvent.focus(block);
      expect(card).toHaveClass("today-plan-linked");
      fireEvent.blur(block);
      expect(card).not.toHaveClass("today-plan-linked");
      // A note task's block links to its card too.
      const memo = within(screen.getByRole("region", { name: "Today's plan" })).getByText("Draft the memo").closest("article")!;
      fireEvent.focus(within(memo as HTMLElement).getByRole("button", { name: /^Draft the memo/ }));
      expect(day.getByRole("button", { name: /^Draft the memo/ })).toHaveAttribute("data-linked", "true");
    });

    it("shows the plan's items in order with the focus, source, goal, booked time, and progress", async () => {
      withPlan();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("block", 0, { title: "Review the contract", start: at(10, 30), end: at(11), sourceTaskId: "focus" }),
      ] });
      renderToday({ calendarConnected: true });

      const plan = await screen.findByRole("region", { name: "Today's plan" });
      const cards = within(plan).getAllByRole("article");
      expect(cards.map((card) => within(card).getByRole("button", { name: /^(Send|Review|Draft|Sam)/ }).textContent)).toEqual([
        expect.stringContaining("Send the deck"), expect.stringContaining("Review the contract"), expect.stringContaining("Draft the memo"), expect.stringContaining("Sam Ortiz"),
      ]);
      expect(cards[1]).toHaveClass("today-plan-focus");
      expect(cards[0]).not.toHaveClass("today-plan-focus");
      expect(within(cards[0]).getByRole("checkbox", { name: "Mark Send the deck not done" })).toBeChecked();
      expect(await within(cards[1]).findByText(/^you@example\.com · Close Q4 deals · 10:30\s?am$/)).toBeInTheDocument();
      expect(within(cards[2]).getByText("Note")).toBeInTheDocument();
      // A check-in has no checkbox; it is done when the contact is marked contacted.
      expect(within(cards[3]).queryByRole("checkbox")).not.toBeInTheDocument();
      expect(screen.getByText("1 of 4 planned done", { selector: ".today-plan-progress" })).toBeInTheDocument();
    });

    it("says nothing is planned and how to add, without a progress line", async () => {
      renderToday();
      expect(await within(screen.getByRole("region", { name: "Today's plan" })).findByText(/^Nothing is planned yet\./)).toBeInTheDocument();
      expect(document.querySelector(".today-plan-progress")).toBeNull();
      expect(mailClient.dailyPlan).toHaveBeenCalledWith("2026-10-28");
    });

    it("checks off a mail task and a note task", async () => {
      withPlan();
      const setTaskStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue(task("focus", { status: "completed" }));
      vi.spyOn(mailClient, "getNote").mockResolvedValue(note("note-7", "v7"));
      const patch = vi.spyOn(mailClient, "patchNoteItem").mockResolvedValue(note("note-7", "v8"));
      const { onTasksChanged } = renderToday();

      fireEvent.click(await screen.findByRole("checkbox", { name: "Mark Review the contract done" }));
      await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("focus", "completed"));
      fireEvent.click(screen.getByRole("checkbox", { name: "Mark Draft the memo done" }));
      await waitFor(() => expect(patch).toHaveBeenCalledWith("note-7", "m1", "v7", true, undefined));
      expect(onTasksChanged).toHaveBeenCalledTimes(2);
      fireEvent.click(screen.getByRole("checkbox", { name: "Mark Send the deck not done" }));
      await waitFor(() => expect(setTaskStatus).toHaveBeenCalledWith("done", "open"));
    });

    it("opens a note task's note, a check-in's contact, and a task for editing", async () => {
      withPlan();
      vi.mocked(mailClient.listTasks).mockResolvedValue([task("focus", { title: "Review the contract" })]);
      const onOpenNote = vi.fn();
      const { onOpenContact, onEditTask } = renderToday({ onOpenNote });
      const plan = within(await screen.findByRole("region", { name: "Today's plan" }));
      fireEvent.click(plan.getByRole("button", { name: /^Draft the memo/ }));
      expect(onOpenNote).toHaveBeenCalledWith("note-7");
      fireEvent.click(plan.getByRole("button", { name: /^Sam Ortiz/ }));
      expect(onOpenContact).toHaveBeenCalledWith("c-sam");
      fireEvent.click(plan.getByRole("button", { name: /^Review the contract/ }));
      await waitFor(() => expect(onEditTask).toHaveBeenCalledWith(expect.objectContaining({ id: "focus" })));
    });

    it("removes an item from the plan with Undo", async () => {
      withPlan();
      const saved = note("daily:2026-10-28", "r2");
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(note("daily:2026-10-28", "r1"));
      const remove = vi.spyOn(mailClient, "removeFromDailyPlan").mockResolvedValue({ note: saved, changed: ["memo"], already: [], previousBody: "## Plan\n\n- old\n" });
      const save = vi.spyOn(mailClient, "saveDailyNote").mockResolvedValue({ saved: true, note: saved } as Awaited<ReturnType<typeof mailClient.saveDailyNote>>);
      const onNotice = vi.fn();
      renderToday({ onNotice });

      fireEvent.click(await screen.findByRole("button", { name: "Remove from Today's Plan: Draft the memo" }));
      await waitFor(() => expect(remove).toHaveBeenCalledWith("2026-10-28", "r1", "memo"));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: "Removed “Draft the memo” from today's plan" })));
      act(() => onNotice.mock.calls.at(-1)![0].undo());
      await waitFor(() => expect(save).toHaveBeenCalledWith("2026-10-28", "## Plan\n\n- old\n", "r2"));
    });

    it("offers Open Note when the plan write is refused", async () => {
      withPlan();
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(note("daily:2026-10-28", "r1"));
      vi.spyOn(mailClient, "removeFromDailyPlan").mockRejectedValue(new Error("That day's note changed. Open it before changing the plan."));
      const onNotice = vi.fn();
      const onOpenNote = vi.fn();
      renderToday({ onNotice, onOpenNote });
      fireEvent.click(await screen.findByRole("button", { name: "Remove from Today's Plan: Draft the memo" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: "That day's note changed. Open it before changing the plan." })));
      onNotice.mock.calls.at(-1)![0].action.run();
      expect(onOpenNote).toHaveBeenCalledWith("daily:2026-10-28");
    });
  });

  describe("Plan Today entry points", () => {
    beforeEach(() => {
      vi.spyOn(mailClient, "planReview").mockImplementation(async (date) => ({ date, reviewed: null, done: [], alsoDone: [], meetings: null, didntGetTo: [] }));
      vi.spyOn(mailClient, "listNoteTasks").mockResolvedValue([]);
    });

    it("offers Plan Today while the plan is empty, and the plan's progress with Edit Plan once it has items", async () => {
      renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Plan Today" }));
      expect(await screen.findByRole("dialog", { name: "Plan Today" })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Edit Plan" })).not.toBeInTheDocument();
      cleanup();

      vi.mocked(mailClient.dailyPlan).mockImplementation(async (date) => ({ ...emptyPlan(date), revision: "r1", items: [planItem("a", { title: "Send the deck" })] }));
      renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Edit Plan" }));
      const dialog = await screen.findByRole("dialog", { name: "Plan Today" });
      // Edit Plan starts from the saved plan.
      expect(await within(dialog).findByRole("button", { name: "Remove: Send the deck" })).toBeInTheDocument();
      expect(mailClient.dailyPlan).toHaveBeenCalledWith(localDate(0));
    });

    it("also offers Plan Tomorrow in the last hour of working hours, for tomorrow's date", async () => {
      const now = new Date();
      const end = Math.min(24 * 60, now.getHours() * 60 + now.getMinutes() + 30);
      const clock = `${String(Math.floor(end / 60)).padStart(2, "0")}:${String(end % 60).padStart(2, "0")}`;
      const evening: WorkingHours = { timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone, workingWindows: [0, 1, 2, 3, 4, 5, 6].map((weekday) => ({ weekday, start: "00:00", end: clock })) };
      renderToday({ workingHours: evening });
      fireEvent.click(await screen.findByRole("button", { name: "Plan Tomorrow" }));
      expect(await screen.findByRole("dialog", { name: "Plan Tomorrow" })).toBeInTheDocument();
      await waitFor(() => expect(mailClient.planReview).toHaveBeenCalledWith(localDate(1)));
      cleanup();
      renderToday({ workingHours: null });
      await screen.findByRole("button", { name: "Plan Today" });
      expect(screen.queryByRole("button", { name: "Plan Tomorrow" })).not.toBeInTheDocument();
    });

    it("opens the routine a palette request asks for, once", async () => {
      const onPlanRequestConsumed = vi.fn();
      renderToday({ planRequest: { day: "tomorrow", id: 7 }, onPlanRequestConsumed });
      expect(await screen.findByRole("dialog", { name: "Plan Tomorrow" })).toBeInTheDocument();
      expect(onPlanRequestConsumed).toHaveBeenCalledWith(7);
    });
  });

  describe("source switcher", () => {
    it("moves between sources with the arrow keys, Home, and End, and remembers the choice", async () => {
      renderToday();
      const tabs = screen.getAllByRole("tab");
      // Starred Mail, Goals, and Notes wait behind More Sources, so the row stays one line.
      expect(tabs.map((tab) => tab.textContent)).toEqual(["Suggested", "Also Due", "Tasks", "You Said You Would", "Keep in Touch"]);
      expect(tabs[0]).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Suggested");
      expect(screen.getByRole("button", { name: "More Sources" })).toHaveAttribute("aria-haspopup", "menu");

      tabs[0].focus();
      fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
      expect(screen.getByRole("tab", { name: "Also Due" })).toHaveFocus();
      fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveFocus();
      // Arrow keys, Home, and End reach the sources behind More Sources too, which join the row while chosen.
      fireEvent.keyDown(document.activeElement!, { key: "End" });
      await waitFor(() => expect(screen.getByRole("tab", { name: "Notes" })).toHaveFocus());
      fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
      expect(screen.getByRole("tab", { name: "Suggested" })).toHaveFocus();
      expect(screen.queryByRole("tab", { name: "Notes" })).not.toBeInTheDocument();
      fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
      await waitFor(() => expect(screen.getByRole("tab", { name: "Notes" })).toHaveAttribute("aria-selected", "true"));
      fireEvent.keyDown(document.activeElement!, { key: "Home" });
      expect(screen.getByRole("tab", { name: "Suggested" })).toHaveAttribute("aria-selected", "true");
      chooseSource("You Said You Would");
      cleanup();

      renderToday();
      expect(screen.getByRole("tab", { name: "You Said You Would" })).toHaveAttribute("aria-selected", "true");
      // Today stays a read surface.
      expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    });

    it("marks tasks already in the plan, leaves them out of Also Due, and adds others to the plan with Undo", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([
        task("planned", { title: "Planned task", dueKind: "date", dueValue: localDate(-1) }),
        task("other", { title: "Other task", dueKind: "date", dueValue: localDate(-1) }),
      ]);
      vi.mocked(mailClient.dailyPlan).mockImplementation(async (date) => ({ ...emptyPlan(date), revision: "r1", items: [planItem("planned", { title: "Planned task" })] }));
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(note("daily", "r1"));
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
      const onNotice = vi.fn();
      renderToday({ onNotice });

      const overdue = within(await screen.findByRole("region", { name: "Overdue" }));
      expect(overdue.queryByText("Planned task")).not.toBeInTheDocument();
      expect(overdue.getByText("Other task")).toBeInTheDocument();
      chooseSource("Tasks");
      const open = within(screen.getByRole("region", { name: "Open tasks" }));
      const planned = open.getByText("Planned task").closest("article")!;
      expect(within(planned as HTMLElement).getByText("In plan")).toBeInTheDocument();
      expect(open.queryByRole("button", { name: "Add to Today's Plan: Planned task" })).not.toBeInTheDocument();

      fireEvent.click(open.getByRole("button", { name: "Add to Today's Plan: Other task" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), "r1", [{ kind: "link", reference: { version: 1, kind: "task", id: "other" }, label: "Other task" }]));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: "Added “Other task” to today's plan", undo: expect.any(Function) })));
    });

    it("turns a promise into a task before adding it", async () => {
      vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([{
        thread: todayThread("thread-p1", { subject: "Report", accountId: "you@example.com" }), messageId: "p1", sentAt: "2026-10-22T10:00:00Z",
        recipients: ["Jill <jill@example.org>"], excerpt: "I'll send the report",
      }]);
      const created = task("made", { title: "I'll send the report" });
      const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue(created);
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
      const { onEditTask } = renderToday({ onNotice: vi.fn() });
      chooseSource("You Said You Would");

      fireEvent.click(await screen.findByRole("button", { name: "Add to Today's Plan: I'll send the report" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "made" }, label: "I'll send the report" }]));
      expect(createTask.mock.invocationCallOrder[0]).toBeLessThan(add.mock.invocationCallOrder[0]);
      expect(onEditTask).toHaveBeenCalledWith(created);
    });

    it("adds a check-in as a link to the contact", async () => {
      vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([contact("ada", "Ada Lovelace", daysFromNow(-3))]);
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
      renderToday({ onNotice: vi.fn() });
      chooseSource("Keep in Touch");
      fireEvent.click(await screen.findByRole("button", { name: "Add to Today's Plan: Ada Lovelace" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "contact", id: "ada@example.com", contact: "ada" }, label: "Ada Lovelace" }]));
    });
  });

  describe("planSources tabs", () => {
    const candidate = (key: string, overrides: Partial<SourceCandidate> = {}): SourceCandidate => ({
      key, title: `Candidate ${key}`, reason: "starred", detail: null, accountId: "you@example.com", due: null, taskId: null, threadId: null,
      contactId: null, goal: null, plannedDays: [], openTasks: null, planned: false,
      action: { kind: "link", addition: { kind: "link", reference: { version: 1, kind: "task", id: key }, label: `Candidate ${key}` } }, ...overrides,
    });
    const saved = () => ({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
    beforeEach(() => { vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null); });
    /** Pages for one source, in order (the last repeats), and nothing for the others, such as Suggested. */
    const serve = (kind: PlanSourceKind, pages: PlanSourcePage[] | Error) => {
      const queue = Array.isArray(pages) ? [...pages] : null;
      return vi.mocked(mailClient.planSources).mockImplementation(async (_date, source) => {
        if (source !== kind) return { next: null, candidates: [] };
        if (!queue) throw pages;
        return queue.length > 1 ? queue.shift()! : queue[0];
      });
    };

    it("reads Starred Mail for the account in view only once its tab is chosen, and marks planned cards", async () => {
      const sources = serve("starredMail", [{ next: null, candidates: [
        candidate("with-task", { title: "Contract review", taskId: "t1", threadId: "th1", detail: "Contract thread" }),
        candidate("planned", { title: "Already planned", planned: true, threadId: "th2" }),
      ] }]);
      const { onEditTask, onOpenThread } = renderToday();
      expect(sources).not.toHaveBeenCalledWith(expect.anything(), "starredMail", expect.anything(), expect.anything());
      chooseSource("Starred Mail");
      const region = within(await screen.findByRole("region", { name: "Starred mail" }));
      expect(sources).toHaveBeenCalledWith(localDate(0), "starredMail", "you@example.com", null);
      expect(region.getByText("Contract thread")).toBeInTheDocument();
      expect(within(region.getByText("Already planned").closest("article")!).getByText("In plan")).toBeInTheDocument();
      expect(region.queryByRole("button", { name: "Add to Today's Plan: Already planned" })).not.toBeInTheDocument();
      fireEvent.click(region.getByRole("button", { name: /^Already planned/ }));
      expect(onOpenThread).toHaveBeenCalledWith("th2");
      vi.mocked(mailClient.listTasks).mockResolvedValue([task("t1", { title: "Contract review" })]);
      fireEvent.click(region.getByRole("button", { name: /^Contract review/ }));
      await waitFor(() => expect(onEditTask).toHaveBeenCalledWith(expect.objectContaining({ id: "t1" })));
    });

    it("adds a starred conversation's task only after the task form saves, and Cancel writes nothing", async () => {
      serve("starredMail", [{ next: null, candidates: [
        candidate("conv", { title: "Board deck", threadId: "th9", action: { kind: "taskForm", threadId: "th9", accountId: "you@example.com" } }),
      ] }]);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
      const mutate = vi.spyOn(mailClient, "mutateThread");
      const mutateMany = vi.spyOn(mailClient, "mutateThreads");
      const onCreateTask = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce(task("made", { title: "Send the board deck" }));
      const { onTasksChanged } = renderToday({ onCreateTask, onNotice: vi.fn() });
      chooseSource("Starred Mail");

      const addButton = await screen.findByRole("button", { name: "Add to Today's Plan: Board deck" });
      fireEvent.click(addButton);
      await waitFor(() => expect(onCreateTask).toHaveBeenCalledWith({ kind: "conversation", threadId: "th9", accountId: "you@example.com" }));
      await act(async () => {});
      expect(add).not.toHaveBeenCalled();
      expect(onTasksChanged).not.toHaveBeenCalled();

      fireEvent.click(addButton);
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "made" }, label: "Send the board deck" }]));
      expect(onTasksChanged).toHaveBeenCalled();
      // Adding never unstars or archives the conversation.
      expect(mutate).not.toHaveBeenCalled();
      expect(mutateMany).not.toHaveBeenCalled();
    });

    it("books a dropped starred conversation only after its task saves", async () => {
      vi.useFakeTimers({ toFake: ["Date"] });
      vi.setSystemTime(new Date(2026, 9, 28, 10, 7));
      try {
        serve("starredMail", [{ next: null, candidates: [
          candidate("conv", { title: "Board deck", threadId: "th9", action: { kind: "taskForm", threadId: "th9", accountId: "you@example.com" } }),
        ] }]);
        const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => ({ id: "booked", accountId: request.accountId, calendarId: request.calendarId, title: request.title, allDay: false, start: request.start, end: request.end }));
        vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
        const onCreateTask = vi.fn().mockResolvedValueOnce(null).mockResolvedValueOnce(task("made", { title: "Send the board deck" }));
        renderToday({ calendarConnected: true, calendarAccounts, workingHours, onCreateTask, onNotice: vi.fn() });
        chooseSource("Starred Mail");
        const card = (await screen.findByText("Board deck")).closest("article") as HTMLElement;
        const column = screen.getByRole("group", { name: "Day view" }).querySelector<HTMLElement>(".calendar-week-column")!;
        const drop = () => {
          document.elementFromPoint = vi.fn(() => column);
          fireEvent.pointerDown(card, { button: 0, pointerId: 4, clientX: 900, clientY: 10 });
          fireEvent.pointerMove(document.body, { pointerId: 4, clientX: 900, clientY: 14 * HOUR_HEIGHT });
          fireEvent.pointerUp(document.body, { pointerId: 4, clientX: 900, clientY: 14 * HOUR_HEIGHT });
        };
        drop();
        await waitFor(() => expect(onCreateTask).toHaveBeenCalledTimes(1));
        await act(async () => {});
        expect(create).not.toHaveBeenCalled();
        drop();
        await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ title: "Send the board deck", sourceTaskId: "made", start: new Date(2026, 9, 28, 14).toISOString() })));
      } finally {
        vi.useRealTimers();
        Reflect.deleteProperty(document, "elementFromPoint");
      }
    });

    it("pages with Show More", async () => {
      const sources = serve("starredMail", [
        { next: "50", candidates: [candidate("one", { title: "First page" })] },
        { next: null, candidates: [candidate("two", { title: "Second page" })] },
      ]);
      renderToday();
      chooseSource("Starred Mail");
      fireEvent.click(await screen.findByRole("button", { name: "Show More" }));
      expect(await screen.findByText("Second page")).toBeInTheDocument();
      expect(screen.getByText("First page")).toBeInTheDocument();
      expect(sources).toHaveBeenLastCalledWith(localDate(0), "starredMail", "you@example.com", "50");
      expect(screen.queryByRole("button", { name: "Show More" })).not.toBeInTheDocument();
    });

    it("lists goals with their tasks, and makes a task for a goal before adding it", async () => {
      const sources = serve("goals", [{ next: null, candidates: [
        candidate("goal:g1", { title: "Close Q4 deals", reason: "goal", detail: "2026-Q4", openTasks: 2, goal: { id: "g1", title: "Close Q4 deals" }, action: { kind: "newTaskForGoal", goalId: "g1", accountId: "you@example.com" } }),
        candidate("gt", { title: "Call Acme", reason: "goalTask", taskId: "gt", goal: { id: "g1", title: "Close Q4 deals" }, plannedDays: ["2026-10-26", "2026-10-27"] }),
      ] }]);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
      const onCreateTask = vi.fn().mockResolvedValue(task("new-goal-task", { title: "Draft the Acme proposal", goalId: "g1" }));
      renderToday({ onCreateTask, onNotice: vi.fn() });
      chooseSource("Goals");
      const region = within(await screen.findByRole("region", { name: "Goals" }));
      expect(sources).toHaveBeenCalledWith(localDate(0), "goals", "you@example.com", null);
      expect(region.getByText("2026-Q4 · 2 open tasks")).toBeInTheDocument();
      expect(region.getByText("Planned Mon, Tue")).toBeInTheDocument();
      fireEvent.click(region.getByRole("button", { name: "New Task for Goal: Close Q4 deals" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "new-goal-task" }, label: "Draft the Acme proposal" }]));
      expect(onCreateTask).toHaveBeenCalledWith({ kind: "goal", goalId: "g1", accountId: "you@example.com" });
      fireEvent.click(region.getByRole("button", { name: "Add to Today's Plan: Call Acme" }));
      await waitFor(() => expect(add).toHaveBeenLastCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "gt" }, label: "Candidate gt" }]));
    });

    it("lists open note tasks from other notes for every account and opens their note", async () => {
      const addition = { kind: "link" as const, reference: { version: 1 as const, kind: "task" as const, id: "m1", note: "note-7" }, label: "Write the agenda" };
      const sources = serve("notes", [{ next: null, candidates: [
        candidate("nt", { title: "Write the agenda", reason: "noteTask", detail: "Team notes", accountId: null, taskId: "note:note-7:m1", action: { kind: "link", addition } }),
      ] }]);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
      const onOpenNote = vi.fn();
      renderToday({ onOpenNote, onNotice: vi.fn() });
      chooseSource("Notes");
      const region = within(await screen.findByRole("region", { name: "Open note tasks" }));
      expect(sources).toHaveBeenCalledWith(localDate(0), "notes", null, null);
      expect(region.getByText("Team notes")).toBeInTheDocument();
      fireEvent.click(region.getByRole("button", { name: /^Write the agenda/ }));
      expect(onOpenNote).toHaveBeenCalledWith("note-7");
      fireEvent.click(region.getByRole("button", { name: "Add to Today's Plan: Write the agenda" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [addition]));
    });

    it("says when a source can't be read", async () => {
      serve("goals", new Error("Database is locked"));
      renderToday();
      chooseSource("Goals");
      expect(await within(screen.getByRole("tabpanel")).findByRole("alert")).toHaveTextContent("Database is locked");
    });
  });

  describe("Suggested and the empty-source fallback", () => {
    const suggestion = (key: string, overrides: Partial<SourceCandidate> = {}): SourceCandidate => ({
      key, title: `Suggestion ${key}`, reason: "overdue", detail: null, accountId: "you@example.com", due: null, taskId: key, threadId: null,
      contactId: null, goal: null, plannedDays: [], openTasks: null, planned: false,
      action: { kind: "link", addition: { kind: "link", reference: { version: 1, kind: "task", id: key }, label: `Suggestion ${key}` } }, ...overrides,
    });
    const saved = () => ({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
    beforeEach(() => { vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null); });

    it("opens on Suggested for the account in view, with each card's reason, and adds one to the plan", async () => {
      const sources = vi.mocked(mailClient.planSources).mockResolvedValue({ next: null, candidates: [
        suggestion("late", { title: "Send the invoice", reason: "overdue" }),
        suggestion("ada", { title: "Ada Lovelace", reason: "keepInTouch", taskId: null, contactId: "c-ada",
          action: { kind: "link", addition: { kind: "link", reference: { version: 1, kind: "contact", id: "ada@example.com", contact: "c-ada" }, label: "Ada Lovelace" } } }),
      ] });
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
      const { onOpenContact } = renderToday({ onNotice: vi.fn() });
      expect(screen.getByRole("tab", { name: "Suggested" })).toHaveAttribute("aria-selected", "true");
      const region = within(await screen.findByRole("region", { name: "Suggested" }));
      expect(sources).toHaveBeenCalledWith(localDate(0), "suggested", "you@example.com", null);
      expect(region.getByText("Overdue")).toBeInTheDocument();
      expect(region.getByText("Keep in Touch")).toBeInTheDocument();
      fireEvent.click(region.getByRole("button", { name: /^Ada Lovelace/ }));
      expect(onOpenContact).toHaveBeenCalledWith("c-ada");
      fireEvent.click(region.getByRole("button", { name: "Add to Today's Plan: Send the invoice" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "late" }, label: "Suggestion late" }]));
    });

    it("turns a suggested promise into a task first, and leaves out promises dismissed on this device", async () => {
      const promise = (id: string, excerpt: string): OpenPromise => ({ thread: todayThread(`thread-${id}`, { subject: "Report", accountId: "you@example.com" }), messageId: id,
        sentAt: "2026-10-22T10:00:00Z", recipients: ["Jill <jill@example.org>"], excerpt });
      vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([promise("p1", "I'll send the report"), promise("p2", "Let me check")]);
      localStorage.clear();
      vi.mocked(mailClient.planSources).mockResolvedValue({ next: null, candidates: [
        suggestion("conv-1", { title: "I'll send the report", reason: "promise", taskId: null, threadId: "thread-p1", action: { kind: "makeTask", threadId: "thread-p1", messageId: "p1" } }),
        suggestion("conv-2", { title: "Let me check", reason: "promise", taskId: null, threadId: "thread-p2", action: { kind: "makeTask", threadId: "thread-p2", messageId: "p2" } }),
      ] });
      const created = task("made", { title: "I'll send the report" });
      vi.spyOn(mailClient, "createTask").mockResolvedValue(created);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue(saved());
      const { onEditTask } = renderToday({ onNotice: vi.fn() });
      chooseSource("You Said You Would");
      fireEvent.click(await screen.findByRole("button", { name: "Dismiss: Let me check" }));
      chooseSource("Suggested");
      const region = within(await screen.findByRole("region", { name: "Suggested" }));
      expect(region.queryByText("Let me check")).not.toBeInTheDocument();
      fireEvent.click(region.getByRole("button", { name: "Add to Today's Plan: I'll send the report" }));
      await waitFor(() => expect(add).toHaveBeenCalledWith(localDate(0), null, [{ kind: "link", reference: { version: 1, kind: "task", id: "made" }, label: "I'll send the report" }]));
      expect(onEditTask).toHaveBeenCalledWith(created);
    });

    it("says an empty Suggested in one line and lends the next source's first cards, with a way to switch", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue(["a", "b", "c", "d"].map((id) => task(id, { dueKind: "date", dueValue: localDate(-1) })));
      renderToday();
      const panel = within(screen.getByRole("tabpanel"));
      expect(await panel.findByText("Nothing to suggest right now.")).toBeInTheDocument();
      const overdue = within(await panel.findByRole("region", { name: "Overdue" }));
      expect(overdue.getAllByRole("article")).toHaveLength(3);
      fireEvent.click(panel.getByRole("button", { name: "Also Due" }));
      expect(screen.getByRole("tab", { name: "Also Due" })).toHaveAttribute("aria-selected", "true");
      expect(within(screen.getByRole("region", { name: "Overdue" })).getAllByRole("article")).toHaveLength(4);
      expect(screen.queryByText("Nothing to suggest right now.")).not.toBeInTheDocument();
    });

    it("lends from the next non-empty source after a remembered empty one, and stays one line when every source is empty", async () => {
      localStorage.setItem("threestrands.today.planSource", "promises");
      vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([contact("ada", "Ada Lovelace", daysFromNow(-3))]);
      renderToday();
      const panel = within(screen.getByRole("tabpanel"));
      expect(await panel.findByText("Nothing you said you would do is waiting.")).toBeInTheDocument();
      expect(await panel.findByRole("region", { name: "Keep in touch" })).toBeInTheDocument();
      expect(panel.getByRole("button", { name: "Keep in Touch" })).toBeInTheDocument();
      cleanup();

      vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
      renderToday();
      expect(await within(screen.getByRole("tabpanel")).findByText("Nothing you said you would do is waiting.")).toBeInTheDocument();
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
      expect(within(screen.getByRole("tabpanel")).queryByRole("region")).not.toBeInTheDocument();
    });

    it("reaches Starred Mail, Goals, and Notes through More Sources by keyboard", async () => {
      renderToday();
      const more = screen.getByRole("button", { name: "More Sources" });
      fireEvent.keyDown(more, { key: "ArrowDown" });
      const menu = await screen.findByRole("menu", { name: "More Sources" });
      expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Starred Mail", "Goals", "Notes"]);
      fireEvent.click(within(menu).getByRole("menuitem", { name: "Goals" }));
      await waitFor(() => expect(screen.getByRole("tab", { name: "Goals" })).toHaveAttribute("aria-selected", "true"));
      expect(within(screen.getByRole("tablist")).getAllByRole("tab")).toHaveLength(6);
    });
  });

  describe("day view", () => {
    beforeEach(() => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(2026, 9, 28, 10, 0)); });
    afterEach(() => { vi.useRealTimers(); });

    it("prepares a meeting, opens anything else in Calendar, and gives meetings a readiness hint", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [
        event("home", 0, { title: "Home", allDay: true, start: localDate(0), end: localDate(1), eventType: "workingLocation" }),
        event("next", 20, { title: "Design sync" }),
        event("later", 120, { title: "Planning" }),
      ], errors: [] });
      vi.spyOn(mailClient, "meetingReadiness").mockImplementation(async (references) => references.map((reference) => ({
        reference, eventAvailable: true, hasOccurrenceNote: reference.id === "next", priorOccurrencesWithNotes: 0, seriesNotes: 0, openCommitments: 0,
        candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, followThrough: "notReviewed", unavailableSections: [], error: null })));
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => <p>Preparation view</p>), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      const { onOpenEvent, rerenderToday } = renderToday({ calendarConnected: true, preparation });

      const day = within(await screen.findByRole("group", { name: "Day view" }));
      const sync = await day.findByRole("button", { name: /^Design sync/ });
      await waitFor(() => expect(sync).toHaveAccessibleDescription("Has notes"));
      expect(sync.querySelector(".calendar-event-mark")).not.toBeNull();
      expect(day.getByRole("button", { name: /^Planning/ })).toHaveAccessibleDescription("No notes yet");
      expect(day.getByRole("button", { name: /^Planning/ }).querySelector(".calendar-event-mark")).toBeNull();

      // The working location is stated once in the header, not listed in the day view.
      expect(day.queryByRole("button", { name: "Home, working location" })).not.toBeInTheDocument();
      expect(screen.getByText("Home", { selector: ".today-day-shape .today-working-location" })).toBeInTheDocument();
      fireEvent.click(sync);
      expect(preparation.onPrepare).toHaveBeenCalledWith(expect.objectContaining({ id: "next" }));
      expect(onOpenEvent).not.toHaveBeenCalled();

      rerenderToday({ calendarConnected: true, preparation: { ...preparation, event: event("next", 20, { title: "Design sync" }) } });
      expect(within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Design sync/ })).toHaveAttribute("aria-expanded", "true");
      // Preparation fills the left column in place of the plan.
      expect(within(screen.getByRole("group", { name: "Preparation" })).getByText("Preparation view")).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Plan" })).not.toBeInTheDocument();
      fireEvent.click(within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Design sync/ }));
      expect(preparation.onClose).toHaveBeenCalledTimes(1);
    });

    it("dims finished meetings, and marks only the meeting being prepared as selected", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [
        event("benefits", -120, { title: "Benefits Review", responseStatus: "needsAction" }),
        event("skip", 60, { title: "Optional talk", responseStatus: "declined" }),
        event("next", 20, { title: "Design sync" }),
      ], errors: [] });
      vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([]);
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => null), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      const { rerenderToday } = renderToday({ calendarConnected: true, preparation });
      const day = within(await screen.findByRole("group", { name: "Day view" }));
      const benefits = await day.findByRole("button", { name: /^Benefits Review/ });
      expect(benefits).toHaveAttribute("data-past", "true");
      expect(benefits).not.toHaveAttribute("data-selected");
      expect(benefits).toHaveAttribute("aria-expanded", "false");
      expect(day.getByRole("button", { name: /^Design sync/ })).not.toHaveAttribute("data-past");
      expect(day.getByRole("button", { name: /^Optional talk/ })).toHaveAttribute("data-response-status", "declined");

      rerenderToday({ calendarConnected: true, preparation: { ...preparation, event: event("next", 20, { title: "Design sync" }) } });
      const sync = within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Design sync/ });
      expect(sync).toHaveAttribute("data-selected", "true");
      expect(within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Benefits Review/ })).not.toHaveAttribute("data-selected");

      // In Today, an unanswered invitation is filled, and only data-selected rings a block.
      const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
      const rules = new Map<string, string>();
      css.walkRules(/^\.today-day-view \.calendar-schedule-event\[/, (rule) => { rules.set(rule.selector, rule.toString()); });
      expect(rules.get(".today-day-view .calendar-schedule-event[data-past=\"true\"]")).toContain("opacity: var(--opacity-dimmed)");
      expect(rules.get(".today-day-view .calendar-schedule-event[data-response-status=\"needsAction\"]")).toContain("color-mix");
      expect(rules.get(".today-day-view .calendar-schedule-event[data-selected=\"true\"]")).toContain("box-shadow");
    });

    it("shows a short meeting's time and its readiness marks at a glance", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [event("ops", 30, { title: "Operations Team Meeting" })], errors: [] });
      vi.spyOn(mailClient, "meetingReadiness").mockImplementation(async (references) => references.map((reference) => ({
        reference, eventAvailable: true, hasOccurrenceNote: true, priorOccurrencesWithNotes: 0, seriesNotes: 0, openCommitments: 2,
        candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, followThrough: "notReviewed", unavailableSections: [], error: null })));
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => null), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      renderToday({ calendarConnected: true, preparation });
      const ops = await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: /^Operations Team Meeting/ });
      expect(ops).toHaveTextContent(/Operations Team Meeting, 10:30\s?AM/i);
      await waitFor(() => expect(ops.querySelector(".calendar-event-mark")).not.toBeNull());
      expect(ops.querySelector(".calendar-event-mark")).toHaveTextContent("2");
      expect(ops.querySelector(".calendar-event-mark svg.lucide-notebook-pen")).not.toBeNull();
      expect(ops).toHaveAccessibleDescription(/Has notes · 2 open commitments/);
    });

    it("opens on the working day, shades the time outside it, and sizes the grid to fit", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
      renderToday({ calendarConnected: true, workingHours });
      const day = await screen.findByRole("group", { name: "Day view" });
      expect(day).toHaveClass("today-day-view-working");
      expect(day.style.getPropertyValue("--today-day-view-grid-h")).toBe(`${8.5 * HOUR_HEIGHT}px`);
      // Ten o'clock is inside working hours, so the top is half an hour before now.
      expect(day.querySelector<HTMLElement>(".calendar-week-scroll")!.scrollTop).toBe(9.5 * HOUR_HEIGHT);
      const shaded = [...day.querySelectorAll<HTMLElement>(".calendar-outside-hours")].map((shade) => [shade.style.top, shade.style.height]);
      expect(shaded).toEqual([["0px", `${9 * HOUR_HEIGHT}px`], [`${17 * HOUR_HEIGHT}px`, `${7 * HOUR_HEIGHT}px`]]);
    });

    it("keeps its old opening and height without working hours", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
      renderToday({ calendarConnected: true });
      const day = await screen.findByRole("group", { name: "Day view" });
      expect(day).not.toHaveClass("today-day-view-working");
      expect(day.querySelector(".calendar-outside-hours")).toBeNull();
    });

    it("prepares the day-view meeting in focus with p", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [event("next", 20, { title: "Design sync" }), event("later", 120, { title: "Planning" })], errors: [] });
      vi.spyOn(mailClient, "meetingReadiness").mockResolvedValue([]);
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => null), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      renderToday({ calendarConnected: true, preparation });
      const planning = await within(screen.getByRole("group", { name: "Day view" })).findByRole("button", { name: /^Planning/ });
      planning.focus();
      act(() => runWorkspaceCommand("meetings.prepare"));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "later" }));
    });

    it("keeps the agenda list in the page as the narrow-width fallback, and hides one or the other with a container query", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [event("next", 20, { title: "Design sync" })], errors: [] });
      renderToday({ calendarConnected: true });
      const day = await screen.findByRole("group", { name: "Day view" });
      const agenda = screen.getByRole("region", { name: "Today's agenda" });
      expect(agenda.closest(".today-agenda-fallback")).not.toBeNull();
      expect(day.closest(".today-agenda-fallback")).toBeNull();
      // The fallback agenda's rows can't be dragged; its meetings keep Prepare and Add Note.
      expect(await within(agenda).findByText("Design sync")).toBeInTheDocument();

      const css = postcss.parse(readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8"));
      const hidden: string[] = [];
      css.walkAtRules("container", (rule) => {
        if (!rule.params.startsWith("today ")) return;
        rule.walkRules((inner) => inner.walkDecls("display", (declaration) => { hidden.push(`${inner.selector}:${declaration.value}`); }));
      });
      expect(hidden).toEqual(expect.arrayContaining([".today-day-view:none", ".today-agenda-fallback:block"]));
      css.walkRules(".today-agenda-fallback", (rule) => rule.walkDecls("display", (declaration) => { if (rule.parent?.type === "root") expect(declaration.value).toBe("none"); }));
    });

    it("says when today's schedule can't be read and offers Try Again", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValue(new Error("offline"));
      vi.spyOn(console, "warn").mockImplementation(() => {});
      renderToday({ calendarConnected: true });
      const day = within(await screen.findByRole("group", { name: "Day view" }));
      expect(await day.findByRole("alert")).toHaveTextContent("free time is unknown");
      expect(day.getByRole("button", { name: "Try Again" })).toBeInTheDocument();
    });
  });

  describe("booking", () => {
    beforeEach(() => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(2026, 9, 28, 10, 7)); });
    afterEach(() => { vi.useRealTimers(); Reflect.deleteProperty(document, "elementFromPoint"); });

    const at = (hours: number, minutes = 0) => new Date(2026, 9, 28, hours, minutes);
    const created = (request: Parameters<typeof mailClient.createCalendarEvent>[0]): ScheduleEvent => ({
      id: "booked", accountId: request.accountId, calendarId: request.calendarId, title: request.title, allDay: false, start: request.start, end: request.end, sourceTaskId: request.sourceTaskId ?? null,
    });
    const withPlan = () => vi.mocked(mailClient.dailyPlan).mockImplementation(async (date) => ({ ...emptyPlan(date), revision: "r1", items: [
      planItem("focus", { title: "Review the contract" }),
      { ...planItem("memo", { title: "Draft the memo", kind: "newTask", taskId: "note:daily:2026-10-28:m1", accountId: null }), key: "memo" },
    ] }));

    async function dragOnto(card: HTMLElement, hours: number) {
      const column = screen.getByRole("group", { name: "Day view" }).querySelector<HTMLElement>(".calendar-week-column")!;
      document.elementFromPoint = vi.fn(() => column);
      fireEvent.pointerDown(card, { button: 0, pointerId: 3, clientX: 900, clientY: 10 });
      fireEvent.pointerMove(document.body, { pointerId: 3, clientX: 900, clientY: hours * HOUR_HEIGHT });
      fireEvent.pointerUp(document.body, { pointerId: 3, clientX: 900, clientY: hours * HOUR_HEIGHT });
    }

    it("books the next free half hour after now with Book, skipping busy time but not declined events", async () => {
      withPlan();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("busy", 0, { title: "Standup", start: at(10, 0).toISOString(), end: at(10, 45).toISOString() }),
        event("no", 0, { title: "Skipped", start: at(10, 45).toISOString(), end: at(11, 30).toISOString(), responseStatus: "declined" }),
      ] });
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      const remove = vi.spyOn(mailClient, "deleteCalendarEvent").mockResolvedValue();
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, workingHours, onNotice });
      await within(screen.getByRole("group", { name: "Day view" })).findByRole("button", { name: /^Standup/ });

      fireEvent.click(await screen.findByRole("button", { name: "Book: Draft the memo" }));
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
        accountId: "cal@example.com", calendarId: "primary", title: "Draft the memo", start: at(10, 45).toISOString(), end: at(11, 15).toISOString(), sourceTaskId: "note:daily:2026-10-28:m1",
      })));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 10:45\s?am for “Draft the memo”$/) })));
      act(() => onNotice.mock.calls.at(-1)![0].undo());
      await waitFor(() => expect(remove).toHaveBeenCalledWith(expect.objectContaining({ id: "booked" })));
    });

    it("says free time is unknown without working hours rather than guessing", async () => {
      withPlan();
      const create = vi.spyOn(mailClient, "createCalendarEvent");
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, onNotice });
      await screen.findByRole("group", { name: "Day view" });
      await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalled());
      fireEvent.click(await screen.findByRole("button", { name: "Book: Review the contract" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "Today's calendar or working hours couldn't be read, so free time is unknown" }));
      expect(create).not.toHaveBeenCalled();
    });

    it("books a drop outside working hours where it lands, and says so", async () => {
      withPlan();
      vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, workingHours, onNotice });
      const card = (await screen.findByText("Review the contract")).closest("article")!;
      await dragOnto(card as HTMLElement, 19);
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 7:00\s?pm \(outside working hours\) for “Review the contract”$/) })));
    });

    it("says so when no free half hour is left today", async () => {
      withPlan();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("all", 0, { title: "Offsite", start: at(10, 0).toISOString(), end: at(23, 59).toISOString() }),
      ] });
      const create = vi.spyOn(mailClient, "createCalendarEvent");
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, workingHours, onNotice });
      await within(screen.getByRole("group", { name: "Day view" })).findByRole("button", { name: /^Offsite/ });
      fireEvent.click(await screen.findByRole("button", { name: "Book: Review the contract" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "No free half hour is left in today's working hours" }));
      expect(create).not.toHaveBeenCalled();
    });

    it("books a plan card dropped on the day view at the drop", async () => {
      withPlan();
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      const add = vi.spyOn(mailClient, "addToDailyPlan");
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, workingHours, onNotice });
      const card = (await screen.findByText("Review the contract")).closest("article")!;
      await dragOnto(card as HTMLElement, 14);
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ title: "Review the contract", start: at(14).toISOString(), sourceTaskId: "focus" })));
      expect(add).not.toHaveBeenCalled();
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 2:00\s?pm for “Review the contract”$/) })));
    });

    it("books a source card dropped on the day view and adds it to the plan", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([task("late", { title: "Late task", dueKind: "date", dueValue: localDate(-1) })]);
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      const add = vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, workingHours, onNotice });
      const card = (await screen.findByText("Late task")).closest("article")!;
      await dragOnto(card as HTMLElement, 15);
      await waitFor(() => expect(add).toHaveBeenCalledWith("2026-10-28", null, [{ kind: "link", reference: { version: 1, kind: "task", id: "late" }, label: "Late task" }]));
      expect(create.mock.invocationCallOrder[0]).toBeLessThan(add.mock.invocationCallOrder[0]);
      expect(create).toHaveBeenCalledWith(expect.objectContaining({ start: at(15).toISOString(), sourceTaskId: "late" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 3:00\s?pm for “Late task” and added it to today's plan$/) })));
    });

    it("keeps the booking and says so with Open Note when the plan refuses the item", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([task("late", { title: "Late task", dueKind: "date", dueValue: localDate(-1) })]);
      vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      const remove = vi.spyOn(mailClient, "deleteCalendarEvent").mockResolvedValue();
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      vi.spyOn(mailClient, "addToDailyPlan").mockRejectedValue(new Error("That day's note changed. Open it before changing the plan."));
      const onNotice = vi.fn();
      const onOpenNote = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, onNotice, onOpenNote });
      const card = (await screen.findByText("Late task")).closest("article")!;
      await dragOnto(card as HTMLElement, 15);
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 3:00\s?pm for “Late task”, but it couldn't be added to today's plan/) })));
      expect(remove).not.toHaveBeenCalled();
      onNotice.mock.calls.at(-1)![0].action.run();
      expect(onOpenNote).toHaveBeenCalledWith("daily:2026-10-28");
    });

    it("books a dropped check-in as an event of its own, with no task", async () => {
      vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([contact("ada", "Ada Lovelace", daysFromNow(-3))]);
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      vi.spyOn(mailClient, "getDailyNote").mockResolvedValue(null);
      vi.spyOn(mailClient, "addToDailyPlan").mockResolvedValue({ note: note("daily", "r2"), changed: ["k"], already: [], previousBody: "" });
      renderToday({ calendarConnected: true, calendarAccounts, onNotice: vi.fn() });
      chooseSource("Keep in Touch");
      const card = (await screen.findByText("Ada Lovelace")).closest(".context-row")!;
      await dragOnto(card as HTMLElement, 16);
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ title: "Check in with Ada Lovelace", start: at(16).toISOString() })));
      expect(create.mock.calls[0][0]).not.toHaveProperty("sourceTaskId");
    });
  });

  describe("agenda", () => {
    // Events are placed minutes from now; near midnight they would spill into tomorrow.
    beforeEach(() => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(2026, 9, 28, 10, 0)); });
    afterEach(() => { vi.useRealTimers(); });

    it("points to Settings instead of loading a schedule when no calendar is connected", async () => {
      renderToday({ calendarConnected: false });
      expect(within(screen.getByRole("region", { name: "Today's agenda" })).getByText(/Connect a calendar in Settings/)).toBeInTheDocument();
      expect(document.querySelector(".today-mail-signal-scope")).toHaveTextContent("you@example.com");
      expect(mailClient.listScheduleEvents).not.toHaveBeenCalled();
      // Without a calendar the header has nothing to say about the day beyond its date.
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
      expect(document.querySelector(".today-day-shape")).toBeNull();
    });

    it("names the account beside the mail line and the sources it scopes, not over the whole page", async () => {
      renderToday({ calendarConnected: true });
      await screen.findByRole("group", { name: "Day view" });
      // The plan and the day view cover every account.
      expect(document.querySelector(".today-header .eyebrow")).toHaveTextContent(/^Today$/);
      expect(within(screen.getByRole("group", { name: "Mail" })).getByText("· you@example.com", { selector: ".today-mail-signal-scope" })).toBeInTheDocument();
      for (const name of ["Suggested", "Also Due", "Tasks", "You Said You Would", "Starred Mail", "Goals"]) {
        chooseSource(name);
        expect(document.querySelector(".today-source-scope"), name).toHaveTextContent("you@example.com");
      }
      for (const name of ["Keep in Touch", "Notes"]) {
        chooseSource(name);
        expect(document.querySelector(".today-source-scope"), name).toBeNull();
      }
    });

    it("states one working location in the header, the next meeting under the summary, and keeps locations out of the day's lists", async () => {
      const place = (id: string, title: string, accountId: string, calendarId: string, day: number) =>
        event(id, 0, { title, accountId, calendarId, allDay: true, start: localDate(day), end: localDate(day + 1), eventType: "workingLocation", workingLocationType: "office" });
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        place("o", "Office", "cal@example.com", "primary", 0),
        place("h", "Home", "other@example.com", "primary", 0),
        place("to", "Office tomorrow", "cal@example.com", "primary", 1),
        event("sync", 30, { title: "Sync" }),
      ] });
      renderToday({ calendarConnected: true, accountId: "cal@example.com", calendars: [...calendars, { id: "primary", accountId: "other@example.com", name: "Home", primary: true, selected: true, writable: true }] });

      const next = await screen.findByRole("button", { name: /^Next: Sync in \d+ min$/ });
      const shape = document.querySelector(".today-day-shape") as HTMLElement;
      expect(shape).toHaveTextContent(/^Office · 1 meeting$/);
      expect(shape.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
      expect(next.closest(".today-heading")).toContainElement(shape);
      expect(next.closest(".today-heading")).not.toContainElement(screen.getByRole("button", { name: "Review Today" }));
      expect(screen.queryByText("Home", { selector: ".context-row-title, .calendar-working-location-name" })).not.toBeInTheDocument();
      const tomorrow = screen.getByRole("region", { name: "Tomorrow" });
      expect(tomorrow.querySelector(".context-section-note")).toHaveTextContent(/ · Office tomorrow$/);
      expect(within(tomorrow).queryByRole("button", { name: /^Office tomorrow/ })).not.toBeInTheDocument();
    });

    it("counts a meeting on two calendars once, and skips declined ones for the count and the next meeting", async () => {
      // The copy reuses the first event's times; two Date.now() reads can land a millisecond apart.
      const sync = event("work", 30, { title: "Sync", accountId: "cal@example.com" });
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        sync,
        { ...sync, id: "copy", responseStatus: "declined" },
        event("skip", 10, { title: "Optional talk", responseStatus: "declined" }),
      ] });
      renderToday({ calendarConnected: true });

      expect(await screen.findByRole("button", { name: /^Next: Sync in \d+ min$/ })).toBeInTheDocument();
      expect(screen.getByText(/^1 meeting( ·|$)/, { selector: ".today-day-shape" })).toBeInTheDocument();
      const agenda = screen.getByRole("region", { name: "Today's agenda" });
      expect(within(agenda).getAllByText("Sync")).toHaveLength(1);
      expect(within(agenda).getByText("Optional talk").closest(".context-row")).toHaveClass("agenda-event-declined");
    });

    it("marks overlapping meetings, notes which is recorded, and describes the rest of the day", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("cs", 5, { title: "CS Updates", description: "This session will be recorded." }),
        event("ramp", 5, { title: "Ramp AI deep dive" }),
        event("later", 40, { title: "Elder meeting" }),
      ] });
      renderToday({ calendarConnected: true });

      const agenda = await screen.findByRole("region", { name: "Today's agenda" });
      const cs = (await within(agenda).findByText("CS Updates")).closest(".context-row")!;
      expect(cs).toHaveClass("agenda-event-conflict");
      expect(within(cs as HTMLElement).getByText(/Overlaps Ramp AI deep dive · Recorded/)).toBeInTheDocument();
      expect(within(agenda).getByText("Elder meeting").closest(".context-row")).not.toHaveClass("agenda-event-conflict");
      expect(screen.getByText(/^3 meetings · double-booked at .+ \(CS Updates is recorded\) · last meeting ends/, { selector: ".today-day-shape" })).toBeInTheDocument();
    });

    it("offers Add Note on the next meeting and today's agenda when it can", async () => {
      const next = event("next", 5, { title: "Design sync" });
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [next] });
      const onAddNote = vi.fn();
      renderToday({ calendarConnected: true, onAddNote });
      await screen.findByRole("button", { name: /^Next: Design sync/ });
      fireEvent.click(screen.getByRole("button", { name: "Add Note" }));
      expect(onAddNote).toHaveBeenLastCalledWith(next);
      fireEvent.click(within(screen.getByRole("region", { name: "Today's agenda" })).getByRole("button", { name: "Add Note for Design sync" }));
      expect(onAddNote).toHaveBeenCalledTimes(2);
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
      // The page heading already gives today's date.
      expect(agenda.querySelector(".context-section-note")).toBeNull();
      const next = await screen.findByRole("button", { name: /^Next: Design sync in \d+ min$/ });
      expect(screen.getByText(/^2 meetings/, { selector: ".today-day-shape" })).toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Join" }));
      expect(openUrl).toHaveBeenCalledWith("https://meet.example.com/design");
      fireEvent.click(next);
      expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "next" }));
    });

    it("shows readiness on agenda rows, prepares from the agenda, the next meeting, and the keyboard, in place of the plan", async () => {
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("past", -60, { title: "Done already" }),
        event("home", 0, { title: "Home", allDay: true, start: localDate(0), end: localDate(1), eventType: "workingLocation" }),
        event("next", 20, { title: "Design sync" }),
        event("later", 120, { title: "Planning" }),
      ] });
      const readiness = vi.spyOn(mailClient, "meetingReadiness").mockImplementation(async (references) => references.map((reference) => ({
        reference, eventAvailable: true, hasOccurrenceNote: false, priorOccurrencesWithNotes: reference.id === "later" ? 1 : 0, seriesNotes: 0, openCommitments: 0,
        candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, followThrough: "notReviewed", unavailableSections: [], error: null })));
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => <p>Preparation view</p>), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      const { rerenderToday, onOpenEvent } = renderToday({ calendarConnected: true, preparation });

      expect(screen.queryByRole("region", { name: /Meeting prep/ })).not.toBeInTheDocument();
      const agenda = within(await screen.findByRole("region", { name: "Today's agenda" }));
      expect(await agenda.findByText(/No notes yet · 1 earlier meeting with notes/)).toBeInTheDocument();
      // Only meetings still ahead are checked; a finished one and a working location aren't.
      await waitFor(() => expect(readiness).toHaveBeenCalled());
      expect(readiness.mock.calls.at(-1)![0].map((reference) => reference.id)).toEqual(["next", "later"]);
      expect(agenda.queryByRole("button", { name: "Prepare for Home" })).not.toBeInTheDocument();
      expect(agenda.queryByRole("button", { name: "Add Note for Home" })).not.toBeInTheDocument();
      expect(agenda.queryByRole("button", { name: /^Home/ })).not.toBeInTheDocument();
      expect(preparation.onPrepare).not.toHaveBeenCalled();

      // On Today, choosing a meeting prepares it rather than leaving for Calendar.
      fireEvent.click(agenda.getByRole("button", { name: /^Planning/ }));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "later" }));
      fireEvent.click(agenda.getByRole("button", { name: "Prepare for Design sync" }));
      fireEvent.click(screen.getByRole("button", { name: "Prepare" }));
      expect(preparation.onPrepare).toHaveBeenCalledTimes(3);
      act(() => runWorkspaceCommand("meetings.prepare"));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "next" }));
      expect(onOpenEvent).not.toHaveBeenCalled();

      const preparing = { ...preparation, event: event("next", 20, { title: "Design sync" }) };
      rerenderToday({ calendarConnected: true, preparation: preparing });
      expect(within(screen.getByRole("group", { name: "Preparation" })).getByText("Preparation view")).toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Today's agenda" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Unread mail" })).not.toBeInTheDocument();
      // Preparation replaces the plan; the schedule stays beside it.
      expect(screen.queryByRole("group", { name: "Plan" })).not.toBeInTheDocument();
      expect(screen.getByRole("group", { name: "Schedule" })).toBeInTheDocument();
      // Preparation has the meeting's actions, so the header card is only its link.
      expect(screen.getByRole("button", { name: /^Next: Design sync/ })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Prepare" })).not.toBeInTheDocument();
      expect(within(screen.getByRole("region", { name: "Today's agenda" })).getByRole("button", { name: /^Design sync/ })).toHaveAttribute("aria-pressed", "true");

      act(() => runWorkspaceCommand("meetings.next"));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "later" }));
      act(() => runWorkspaceCommand("meetings.closePrepare"));
      expect(preparation.onClose).toHaveBeenCalledTimes(1);
      // Choosing the meeting being prepared again closes it.
      fireEvent.click(within(screen.getByRole("region", { name: "Today's agenda" })).getByRole("button", { name: /^Design sync/ }));
      expect(preparation.onClose).toHaveBeenCalledTimes(2);
    });
  });
  describe("evening view", () => {
    const clock = (day: number, hours: number, minutes = 0) => vi.setSystemTime(new Date(2026, 10, day, hours, minutes));
    const meetingAt = (id: string, day: number, start: number, end: number, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
      id, accountId: "cal@example.com", calendarId: "primary", title: `Event ${id}`, allDay: false,
      start: new Date(2026, 10, day, start).toISOString(), end: new Date(2026, 10, day, end).toISOString(), ...overrides,
    });
    const schedule = (...events: ScheduleEvent[]) => vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events, errors: [] });
    const unreviewed = () => vi.spyOn(mailClient, "meetingReadiness").mockImplementation(async (references) => references.map((reference) => ({
      reference, eventAvailable: true, hasOccurrenceNote: false, priorOccurrencesWithNotes: 0, seriesNotes: 0, openCommitments: 0,
      candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, followThrough: "notReviewed" as const, unavailableSections: [], error: null })));
    const evening = () => screen.findByRole("group", { name: "Today in review" });
    const preparation = () => ({ event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => <p>Preparation view</p>), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() });

    it("opens by itself only once the last meeting has ended and working hours are over", async () => {
      schedule(meetingAt("sync", 17, 17, 18, { start: new Date(2026, 10, 17, 15, 0).toISOString(), end: new Date(2026, 10, 17, 17, 30).toISOString() }));
      // Hours over (17:00) but the meeting runs to 17:30: still the day.
      clock(17, 17, 10);
      renderToday({ calendarConnected: true, calendars, workingHours });
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Review Today" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Today in review" })).not.toBeInTheDocument();
      cleanup();
      clock(17, 17, 31);
      renderToday({ calendarConnected: true, calendars, workingHours });
      expect(await evening()).toBeInTheDocument();
      expect(screen.getByRole("group", { name: "Tomorrow" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Plan" })).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Back to Today's Plan" })).toBeInTheDocument();
      // Before working hours end, an idle day stays the day.
      cleanup();
      schedule();
      clock(17, 16, 30);
      renderToday({ calendarConnected: true, calendars, workingHours });
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
    });

    it("waits for 6 pm without working hours, with or without a calendar, and never opens while the schedule is unknown", async () => {
      clock(17, 17, 59);
      renderToday();
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      cleanup();
      clock(17, 18, 1);
      renderToday();
      expect(await evening()).toBeInTheDocument();
      cleanup();
      vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValue(new Error("offline"));
      renderToday({ calendarConnected: true, calendars });
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Today in review" })).not.toBeInTheDocument();
    });

    it("lets Review Today and Back to Today's Plan choose the view for the rest of the day on this device", async () => {
      clock(17, 10);
      renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Review Today" }));
      expect(await evening()).toBeInTheDocument();
      cleanup();
      // Remembered across a restart the same day.
      renderToday();
      expect(await evening()).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Back to Today's Plan" }));
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      cleanup();
      // After hours, the choice to go back still holds, until the day turns over.
      clock(17, 20);
      renderToday();
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      cleanup();
      clock(18, 20);
      renderToday();
      expect(await evening()).toBeInTheDocument();
    });

    it("toggles the evening view from the palette's Review Today, once per request", async () => {
      const onPlanRequestConsumed = vi.fn();
      clock(17, 10);
      const { rerenderToday } = renderToday({ planRequest: { day: "review", id: 3 }, onPlanRequestConsumed });
      expect(await evening()).toBeInTheDocument();
      expect(onPlanRequestConsumed).toHaveBeenCalledWith(3);
      rerenderToday({ planRequest: { day: "review", id: 4 }, onPlanRequestConsumed });
      expect(await screen.findByRole("group", { name: "Plan" })).toBeInTheDocument();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("shows tomorrow's carried items and tomorrow's day view, and Plan Tomorrow opens the routine for tomorrow with them chosen", async () => {
      clock(17, 19);
      const carried = planItem("memo", { title: "Draft the memo", marker: { kind: "deferred", date: localDate(1) } });
      vi.mocked(mailClient.dailyPlan).mockImplementation(async (date) => ({ ...emptyPlan(date), revision: "r1", notToday: date === localDate(0) ? [carried] : [] }));
      vi.spyOn(mailClient, "planReview").mockImplementation(async (date) => ({ date, reviewed: localDate(0), done: [], alsoDone: [], meetings: null,
        didntGetTo: date === localDate(1) ? [{ ...carried, plannedOn: localDate(0) }] : [] }));
      schedule(meetingAt("standup", 18, 9, 10, { title: "Standup" }));
      unreviewed();
      renderToday({ calendarConnected: true, calendars, workingHours });
      const tomorrow = within(await screen.findByRole("group", { name: "Tomorrow" }));
      expect(await tomorrow.findByRole("button", { name: "Draft the memo" })).toBeInTheDocument();
      expect(await within(tomorrow.getByRole("group", { name: "Tomorrow's day view" })).findByRole("button", { name: /^Standup/ })).toBeInTheDocument();
      // Today's decided carry isn't a loose end, and the header leaves Plan Tomorrow to this column.
      expect(within(await evening()).queryByRole("button", { name: "Carry to Tomorrow" })).not.toBeInTheDocument();
      const header = within(document.querySelector(".today-header") as HTMLElement);
      expect(header.queryByRole("button", { name: "Plan Tomorrow" })).not.toBeInTheDocument();
      fireEvent.click(tomorrow.getByRole("button", { name: "Plan Tomorrow" }));
      const dialog = await screen.findByRole("dialog", { name: "Plan Tomorrow" });
      await waitFor(() => expect(mailClient.planReview).toHaveBeenCalledWith(localDate(1)));
      expect(await within(dialog).findByRole("button", { name: "Carry" })).toHaveAttribute("aria-pressed", "true");
    });

    it("decides a loose end from the left column with the app's notice and Undo, and shows Day closed once everything is decided", async () => {
      clock(17, 19);
      const row = { ...planItem("memo", { title: "Draft the memo" }), plannedOn: localDate(0) };
      const read = vi.spyOn(mailClient, "planReview").mockResolvedValue({ date: localDate(1), reviewed: localDate(0), done: [], alsoDone: [], meetings: null, didntGetTo: [row] });
      vi.spyOn(mailClient, "saveDailyPlan").mockResolvedValue({ operationId: "op", note: note("daily:x", "r2"), added: [], removed: [], notToday: [row.key], already: [], changed: true, shutdown: false });
      const onNotice = vi.fn();
      renderToday({ onNotice });
      const review = within(await evening());
      expect(screen.queryByText("Day closed")).not.toBeInTheDocument();
      read.mockResolvedValue({ date: localDate(1), reviewed: localDate(0), done: [], alsoDone: [], meetings: null, didntGetTo: [] });
      fireEvent.click(await review.findByRole("button", { name: "Carry to Tomorrow" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: "Carried “Draft the memo” to tomorrow", undo: expect.any(Function) })));
      expect(await screen.findByText("Day closed")).toBeInTheDocument();
    });

    it("keeps preparation in the left column in place of the review, and p prepares tomorrow's first meeting when today's are over", async () => {
      clock(17, 19);
      schedule(meetingAt("done", 17, 9, 10), meetingAt("standup", 18, 9, 10, { title: "Standup" }));
      unreviewed();
      const prep = preparation();
      const { rerenderToday } = renderToday({ calendarConnected: true, calendars, workingHours, preparation: prep });
      await evening();
      act(() => runWorkspaceCommand("meetings.prepare"));
      expect(prep.onPrepare).toHaveBeenCalledWith(expect.objectContaining({ id: "standup" }));
      rerenderToday({ calendarConnected: true, calendars, workingHours, preparation: { ...prep, event: meetingAt("standup", 18, 9, 10, { title: "Standup" }) } });
      expect(within(screen.getByRole("group", { name: "Preparation" })).getByText("Preparation view")).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Today in review" })).not.toBeInTheDocument();
      expect(screen.getByRole("group", { name: "Tomorrow" })).toBeInTheDocument();
      expect(within(screen.getByRole("group", { name: "Tomorrow's day view" })).getByRole("button", { name: /^Standup/ })).toHaveAttribute("aria-expanded", "true");
      act(() => runWorkspaceCommand("meetings.closePrepare"));
      expect(prep.onClose).toHaveBeenCalled();
    });

    it("draws the backdrop behind the header only when Today Backdrop is on, for the view shown", async () => {
      clock(17, 10);
      renderToday();
      await screen.findByRole("group", { name: "Plan" });
      expect(document.querySelector(".today-backdrop")).toBeNull();
      cleanup();
      localStorage.setItem("threestrands.today.backdrop", "on");
      renderToday();
      await screen.findByRole("group", { name: "Plan" });
      const backdrop = document.querySelector(".today-header > .today-backdrop");
      expect(backdrop).toHaveAttribute("data-view", "day");
      expect(backdrop).toHaveAttribute("aria-hidden", "true");
      fireEvent.click(screen.getByRole("button", { name: "Review Today" }));
      await evening();
      expect(document.querySelector(".today-header > .today-backdrop")).toHaveAttribute("data-view", "evening");
    });
  });
});

describe("TodayWorkspace tomorrow", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([]);
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
      onOpenTasks={vi.fn()} onOpenContact={vi.fn()} onOpenKeepInTouch={vi.fn()} onOpenCalendar={vi.fn()} />);

    const tomorrow = await screen.findByRole("region", { name: "Tomorrow" });
    expect(await within(tomorrow).findByText("Dentist")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Today's agenda" })).queryByText("Dentist")).not.toBeInTheDocument();
    expect(screen.getByText(/^No meetings today$/, { selector: ".today-day-shape" })).toBeInTheDocument();
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
