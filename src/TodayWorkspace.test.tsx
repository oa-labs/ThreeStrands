import { refreshSyncedData } from "./refreshSyncedData";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ContactProfile, DailyPlan, DailyPlanItem, Note, OpenPromise, ScheduleEvent, ThreadTask, TodayMail } from "./domain";
import { HOUR_HEIGHT } from "./calendarTime";
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
    key: JSON.stringify(["task", null, null, null, key]), kind: "task", reference: { version: 1, kind: "task", id: key }, title: `Task ${key}`,
    state: "open", marker: null, taskId: key, accountId: "you@example.com", goal: null, contactId: null, start: 0, end: 0, ...overrides,
  };
}

function note(id: string, updatedAt = "v1"): Note {
  return { id, title: id, body: "", dailyDate: null, createdAt: updatedAt, updatedAt, trashed: false, trashedAt: null };
}

const calendarAccounts: CalendarAccount[] = [{ email: "cal@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];

const chooseSource = (name: string) => fireEvent.click(screen.getByRole("tab", { name }));

describe("TodayWorkspace", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([]);
    vi.spyOn(mailClient, "listOpenPromises").mockResolvedValue([]);
    vi.spyOn(mailClient, "dailyPlan").mockImplementation(async (date) => emptyPlan(date));
    mockMail([]);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); openUrl.mockReset(); localStorage.clear(); });

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
      expect(screen.getByText("· All accounts", { selector: ".eyebrow-account" })).toBeInTheDocument();
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

    it("opens the Inbox, Calendar, and Tasks from Today", async () => {
      const { onOpenInbox, onOpenCalendar, onOpenTasks } = renderToday();
      fireEvent.click(await screen.findByRole("button", { name: "Open Inbox" }));
      expect(onOpenInbox).toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Open Calendar" }));
      expect(onOpenCalendar).toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "Open Tasks" }));
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

      // Also Due is the default source.
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

  describe("source switcher", () => {
    it("moves between sources with the arrow keys, Home, and End, and remembers the choice", async () => {
      renderToday();
      const tabs = screen.getAllByRole("tab");
      expect(tabs.map((tab) => tab.textContent)).toEqual(["Also Due", "Tasks", "You Said You Would", "Keep in Touch"]);
      expect(tabs[0]).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tabpanel")).toHaveAccessibleName("Also Due");

      tabs[0].focus();
      fireEvent.keyDown(tabs[0], { key: "ArrowRight" });
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveAttribute("aria-selected", "true");
      expect(screen.getByRole("tab", { name: "Tasks" })).toHaveFocus();
      fireEvent.keyDown(document.activeElement!, { key: "End" });
      expect(screen.getByRole("tab", { name: "Keep in Touch" })).toHaveFocus();
      fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
      expect(screen.getByRole("tab", { name: "Also Due" })).toHaveFocus();
      fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
      expect(screen.getByRole("tab", { name: "Keep in Touch" })).toHaveAttribute("aria-selected", "true");
      fireEvent.keyDown(document.activeElement!, { key: "Home" });
      expect(screen.getByRole("tab", { name: "Also Due" })).toHaveAttribute("aria-selected", "true");
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
        candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, unavailableSections: [], error: null })));
      const preparation = { event: null as ScheduleEvent | null, onPrepare: vi.fn(), onClose: vi.fn(), render: vi.fn(() => <p>Preparation view</p>), ownAddresses: [], onOpenNote: vi.fn(), onNotice: vi.fn() };
      const { onOpenEvent, rerenderToday } = renderToday({ calendarConnected: true, preparation });

      const day = within(await screen.findByRole("group", { name: "Day view" }));
      const sync = await day.findByRole("button", { name: /^Design sync/ });
      await waitFor(() => expect(sync).toHaveAccessibleDescription("Has notes"));
      expect(sync.querySelector(".calendar-event-mark")).not.toBeNull();
      expect(day.getByRole("button", { name: /^Planning/ })).toHaveAccessibleDescription("No notes yet");
      expect(day.getByRole("button", { name: /^Planning/ }).querySelector(".calendar-event-mark")).toBeNull();

      fireEvent.click(day.getByRole("button", { name: "Home, working location" }));
      expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "home" }));
      fireEvent.click(sync);
      expect(preparation.onPrepare).toHaveBeenCalledWith(expect.objectContaining({ id: "next" }));
      expect(onOpenEvent).toHaveBeenCalledTimes(1);

      rerenderToday({ calendarConnected: true, preparation: { ...preparation, event: event("next", 20, { title: "Design sync" }) } });
      expect(within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Design sync/ })).toHaveAttribute("aria-expanded", "true");
      // Preparation fills the left column in place of the plan.
      expect(within(screen.getByRole("group", { name: "Preparation" })).getByText("Preparation view")).toBeInTheDocument();
      expect(screen.queryByRole("group", { name: "Plan" })).not.toBeInTheDocument();
      fireEvent.click(within(screen.getByRole("group", { name: "Day view" })).getByRole("button", { name: /^Design sync/ }));
      expect(preparation.onClose).toHaveBeenCalledTimes(1);
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
      renderToday({ calendarConnected: true, calendarAccounts, onNotice });
      await within(screen.getByRole("group", { name: "Day view" })).findByRole("button", { name: /^Standup/ });

      fireEvent.click(await screen.findByRole("button", { name: "Book: Draft the memo" }));
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
        accountId: "cal@example.com", calendarId: "primary", title: "Draft the memo", start: at(10, 45).toISOString(), end: at(11, 15).toISOString(), sourceTaskId: "note:daily:2026-10-28:m1",
      })));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringMatching(/^Booked 10:45\s?am for “Draft the memo”$/) })));
      act(() => onNotice.mock.calls.at(-1)![0].undo());
      await waitFor(() => expect(remove).toHaveBeenCalledWith(expect.objectContaining({ id: "booked" })));
    });

    it("says so when no free half hour is left today", async () => {
      withPlan();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ errors: [], events: [
        event("all", 0, { title: "Offsite", start: at(10, 0).toISOString(), end: at(23, 59).toISOString() }),
      ] });
      const create = vi.spyOn(mailClient, "createCalendarEvent");
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, onNotice });
      await within(screen.getByRole("group", { name: "Day view" })).findByRole("button", { name: /^Offsite/ });
      fireEvent.click(await screen.findByRole("button", { name: "Book: Review the contract" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "No free half hour is left today" }));
      expect(create).not.toHaveBeenCalled();
    });

    it("books a plan card dropped on the day view at the drop", async () => {
      withPlan();
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockImplementation(async (request) => created(request));
      const add = vi.spyOn(mailClient, "addToDailyPlan");
      const onNotice = vi.fn();
      renderToday({ calendarConnected: true, calendarAccounts, onNotice });
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
      renderToday({ calendarConnected: true, calendarAccounts, onNotice });
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
      expect(screen.getByText("· you@example.com", { selector: ".eyebrow-account" })).not.toHaveAttribute("title");
      expect(mailClient.listScheduleEvents).not.toHaveBeenCalled();
      // Without a calendar the header has nothing to say about the day beyond its date.
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalled());
      expect(document.querySelector(".today-day-shape")).toBeNull();
    });

    it("names the account like other views and says the day view covers every calendar", async () => {
      renderToday({ calendarConnected: true });
      expect(await screen.findByText("· you@example.com", { selector: ".eyebrow-account" }))
        .toHaveAttribute("title", "Mail and tasks follow this account. The day view shows all your calendars.");
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
      expect(screen.getByText(/^3 meetings · Double-booked at .+ \(CS Updates is recorded\) · Last meeting ends/, { selector: ".today-day-shape" })).toBeInTheDocument();
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
        candidateCommitments: 0, latestCorrespondenceAt: null, scopeRequired: false, unavailableSections: [], error: null })));
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
      fireEvent.click(agenda.getByRole("button", { name: /^Home/ }));
      expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "home" }));
      expect(preparation.onPrepare).not.toHaveBeenCalled();

      // On Today, choosing a meeting prepares it rather than leaving for Calendar.
      fireEvent.click(agenda.getByRole("button", { name: /^Planning/ }));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "later" }));
      fireEvent.click(agenda.getByRole("button", { name: "Prepare for Design sync" }));
      fireEvent.click(screen.getByRole("button", { name: "Prepare" }));
      expect(preparation.onPrepare).toHaveBeenCalledTimes(3);
      act(() => runWorkspaceCommand("meetings.prepare"));
      expect(preparation.onPrepare).toHaveBeenLastCalledWith(expect.objectContaining({ id: "next" }));
      expect(onOpenEvent).toHaveBeenCalledTimes(1);

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
    expect(screen.getByText(/^No meetings$/, { selector: ".today-day-shape" })).toBeInTheDocument();
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
