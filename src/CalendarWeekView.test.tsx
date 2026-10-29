import { installTestShortcuts } from "./test/commandContext";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { COLLAPSED_CALENDAR_ACCOUNTS_KEY, CalendarWeekView, monthGridDays } from "./CalendarWeekView";
import { DONE_TASK_LINGER_MS } from "./CalendarTasks";
import { startOfLocalDay } from "./calendarTime";
import { mailClient } from "./data/client";
import { clearScheduleCache } from "./calendarScheduleCache";
import { CALENDAR_COLORS_KEY, resetCalendarColorsForTests } from "./calendarColors";
import { expectSharedButtons } from "./test/sharedButtons";
import { expectContextRows } from "./test/contextRows";
import type { CalendarAccount, CalendarOption, ScheduleEvent, ThreadTask } from "./domain";

const accounts: CalendarAccount[] = [
  { email: "joel@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "connected" },
];
const calendars: CalendarOption[] = [
  { id: "primary", accountId: "joel@example.com", name: "joel@example.com", primary: true, selected: true, writable: true },
  { id: "holidays", accountId: "joel@example.com", name: "Holidays in United States", primary: false, selected: true, writable: false },
];

type WeekViewOptions = Omit<Parameters<typeof CalendarWeekView>[0], "anchor" | "onAnchorChange">;

function StatefulWeekView(props: WeekViewOptions) {
  const [anchor, setAnchor] = useState(() => startOfLocalDay(new Date()));
  return <CalendarWeekView {...props} anchor={anchor} onAnchorChange={setAnchor} />;
}

function renderWeek(overrides: Partial<WeekViewOptions> = {}) {
  const props = {
    accounts,
    calendars,
    onToggleCalendar: vi.fn(),
    onAddCalendarAccount: vi.fn(),
    onOpenSettings: vi.fn(),
    ...overrides,
  };
  const result = render(<StatefulWeekView {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "Calendars" }));
  return { ...result, props };
}

function calendarGrid() {
  return within(document.querySelector(".calendar-week-main") as HTMLElement);
}

function rangeButton(name: string) {
  return within(screen.getByRole("group", { name: "Calendar Range" })).getByRole("button", { name });
}

function event(id: string, start: string, end: string, title = id): ScheduleEvent {
  return { id, accountId: "joel@example.com", title, start, end, allDay: false };
}

describe("CalendarWeekView", () => {
  beforeEach(() => {
    localStorage.setItem("threestrands.calendar.range", "week");
    vi.setSystemTime(new Date(2026, 8, 22, 14, 30));
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "listTasks").mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    clearScheduleCache();
    vi.useRealTimers();
    vi.restoreAllMocks();
    localStorage.clear();
    resetCalendarColorsForTests();
  });

  it("returns to today with t and opens a new event with Shift+N, leaving dialogs and editing fields alone", async () => {
    const onCreated = vi.fn();
    renderWeek({ onCreated });
    const toggleSchedule = vi.fn();
    installTestShortcuts({ focusedPane: "calendar", calendarWeekActive: true, toggleSchedule });
    fireEvent.keyDown(document.body, { key: "=" });
    fireEvent.keyDown(document.body, { key: "t" });
    expect(toggleSchedule).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "September 2026", level: 1 })).toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "N", shiftKey: true });
    const dialog = screen.getByRole("dialog", { name: "New event" });
    const title = within(dialog).getByRole("textbox", { name: "Title" });
    fireEvent.change(title, { target: { value: "Meeting" } });
    fireEvent.keyDown(title, { key: "N", shiftKey: true });
    fireEvent.keyDown(title, { key: "t" });
    expect(title).toHaveValue("Meeting");
    expect(screen.getAllByRole("dialog", { name: "New event" })).toHaveLength(1);
    await waitFor(() => expect(onCreated).not.toHaveBeenCalled());
  });
  it("shows today's full agenda in shared context rows, with all-day events before chronological meetings", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [
      event("late", "2026-09-22T15:00:00", "2026-09-22T16:00:00", "Afternoon review"),
      event("tomorrow", "2026-09-23T09:00:00", "2026-09-23T10:00:00", "Tomorrow"),
      event("early", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Morning review"),
      { ...event("all", "2026-09-21", "2026-09-23", "Company offsite"), allDay: true },
      event("overnight", "2026-09-21T23:00:00", "2026-09-22T01:00:00", "Overnight maintenance"),
      event("noon", "2026-09-22T12:00:00", "2026-09-22T13:00:00", "Lunch"),
    ], errors: [] });
    renderWeek();
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    await within(agenda).findByRole("button", { name: /Morning review/ });
    expectContextRows(agenda);
    expect(agenda).toHaveTextContent("Tuesday, September 22");
    expect([...agenda.querySelectorAll(".context-row-title")].map((row) => row.textContent)).toEqual([
      "Company offsite", "Overnight maintenance", "Morning review", "Lunch", "Afternoon review",
    ]);
    expect(within(agenda).getByRole("button", { name: /Company offsite/ })).toHaveTextContent("All day");
    expect(within(agenda).getByRole("button", { name: /Morning review/ })).toHaveTextContent("9:00–10:00 am");
    expect(within(agenda).queryByText("Tomorrow")).not.toBeInTheDocument();
    expect(within(agenda).queryByRole("button", { name: /Show .* more/ })).not.toBeInTheDocument();
    await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalledTimes(3));
  });

  it("opens agenda items in meeting details and restores the agenda when details close", async () => {
    const meeting = event("review", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Project review");
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [meeting], errors: [] });
    const { container } = renderWeek();
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    fireEvent.click(await within(agenda).findByRole("button", { name: /Project review/ }));
    const details = await screen.findByRole("region", { name: "Project review details" });
    expect(details).toHaveFocus();
    // The heading's glyph column holds an icon, so the label lines up with the rows below.
    const heading = screen.getByRole("heading", { name: "Event details" });
    expect(heading.querySelector(".context-row-glyph svg")).not.toBeNull();
    expect(screen.queryByRole("region", { name: "Today's agenda" })).not.toBeInTheDocument();
    expect(container.querySelector(".calendar-week-scroll")?.scrollTop).toBeGreaterThan(0);
    fireEvent.click(within(details).getByRole("button", { name: "Close Event Details" }));
    expect(await screen.findByRole("region", { name: "Today's agenda" })).toHaveTextContent("Project review");
    expect(screen.getByRole("complementary", { name: "Meeting context" })).toHaveFocus();
  });

  it("keeps today's agenda while browsing another week and returns the grid to today on selection", async () => {
    const meeting = event("review", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Today's review");
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [meeting], errors: [] });
    renderWeek();
    await within(screen.getByRole("region", { name: "Today's agenda" })).findByRole("button", { name: /Today's review/ });
    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    expect(screen.getByText("Sun 27")).toBeInTheDocument();
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    fireEvent.click(await within(agenda).findByRole("button", { name: /Today's review/ }));
    expect(await screen.findByRole("region", { name: "Today's review details" })).toHaveFocus();
    expect(screen.getByText("Tue 22")).toBeInTheDocument();
    expect(rangeButton("Week")).toHaveAttribute("aria-pressed", "true");
  });

  it("opens a weekend agenda item without overwriting the saved Workweek preference", async () => {
    vi.setSystemTime(new Date(2026, 8, 26, 10));
    localStorage.setItem("threestrands.calendar.range", "workweek");
    const meeting = event("weekend", "2026-09-26T11:00:00", "2026-09-26T12:00:00", "Saturday review");
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [meeting], errors: [] });
    renderWeek();
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    expect(screen.getByText("Mon 28")).toBeInTheDocument();
    fireEvent.click(await within(agenda).findByRole("button", { name: /Saturday review/ }));
    expect(await screen.findByRole("region", { name: "Saturday review details" })).toBeInTheDocument();
    expect(screen.getByText("Sat 26")).toBeInTheDocument();
    expect(rangeButton("Day")).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("threestrands.calendar.range")).toBe("workweek");
  });

  it("shows an empty agenda and explains when no calendars are selected", async () => {
    const { unmount } = renderWeek();
    expect(await within(screen.getByRole("region", { name: "Today's agenda" })).findByText("No events today.")).toBeInTheDocument();
    unmount();
    renderWeek({ calendars: calendars.map((calendar) => ({ ...calendar, selected: false })) });
    expect(await screen.findByText("Select a calendar to see today's agenda.")).toBeInTheDocument();
  });

  it("respects selected calendars and distinguishes same-id events across accounts and calendars", async () => {
    const meeting = { ...event("shared", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Work review"), calendarId: "primary", location: "Room A", responseStatus: "needsAction" as const };
    localStorage.setItem(CALENDAR_COLORS_KEY, JSON.stringify({ "joel@example.com": { primary: "teal" } }));
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [
      meeting,
      { ...meeting, calendarId: "holidays", title: "Hidden calendar" },
      { ...meeting, calendarId: "team", title: "Team review" },
      { ...meeting, accountId: "other@example.com", title: "Other review" },
    ], errors: [] });
    renderWeek({ calendars: [
      calendars[0], { ...calendars[1], selected: false },
      { ...calendars[0], id: "team", name: "Team" },
      { ...calendars[0], accountId: "other@example.com", name: "Other" },
    ] });
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    const work = await within(agenda).findByRole("button", { name: /Work review/ });
    expect(work).toHaveTextContent("Room A");
    expect(work).toHaveTextContent("Awaiting response");
    expect(work).toHaveTextContent("joel@example.com");
    expect(work.querySelector("svg")?.style.getPropertyValue("--calendar-color")).toBe("#1f9a8f");
    expect(within(agenda).getByRole("button", { name: /Team review/ })).toBeInTheDocument();
    expect(within(agenda).getByRole("button", { name: /Other review/ })).toHaveTextContent("other@example.com");
    expect(within(agenda).queryByText("Hidden calendar")).not.toBeInTheDocument();
  });

  it("loads and retries today's agenda independently when the displayed week doesn't include today", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const dayStart = new Date(2026, 8, 22).toISOString();
    let retry = false;
    vi.mocked(mailClient.listScheduleEvents).mockImplementation(async (start) => {
      if (start === dayStart) {
        if (!retry) throw new Error("offline");
        return { events: [event("today", "2026-09-22T12:00:00", "2026-09-22T13:00:00", "Recovered meeting")], errors: [] };
      }
      return { events: [], errors: [] };
    });
    renderWeek();
    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    const button = await within(agenda).findByRole("button", { name: "Retry agenda" });
    expect(agenda).not.toHaveTextContent("No events today.");
    retry = true;
    fireEvent.click(button);
    expect(await within(agenda).findByRole("button", { name: /Recovered meeting/ })).toBeInTheDocument();
    expect(screen.getByText("Sun 27")).toBeInTheDocument();
    expect(mailClient.listScheduleEvents).toHaveBeenCalledWith(dayStart, new Date(2026, 8, 23).toISOString(), expect.any(String));
  });

  it("rolls today's agenda forward at local midnight without navigating the grid", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 22, 23, 59, 30));
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [
      event("today", "2026-09-22T12:00:00", "2026-09-22T13:00:00", "Tuesday meeting"),
      event("tomorrow", "2026-09-23T12:00:00", "2026-09-23T13:00:00", "Wednesday meeting"),
    ], errors: [] });
    renderWeek();
    await act(async () => {});
    expect(screen.getByRole("region", { name: "Today's agenda" })).toHaveTextContent("Tuesday meeting");
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    const agenda = screen.getByRole("region", { name: "Today's agenda" });
    expect(agenda).toHaveTextContent("Wednesday, September 23");
    expect(agenda).toHaveTextContent("Wednesday meeting");
    expect(agenda).not.toHaveTextContent("Tuesday meeting");
    expect(screen.getByText("Sun 20")).toBeInTheDocument();
  });

  it("shows the seven Sunday-anchored days of the current week with the month title", async () => {
    renderWeek();
    await screen.findByRole("heading", { level: 1, name: "September 2026" });
    for (const label of ["Sun 20", "Mon 21", "Tue 22", "Wed 23", "Thu 24", "Fri 25", "Sat 26"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("says all calendar accounts in the header when every connected account contributes, naming them in the tooltip", () => {
    const { container } = renderWeek({
      accounts: [...accounts, { ...accounts[0], email: "work@example.com" }],
      calendars: [...calendars, { id: "work", accountId: "work@example.com", name: "Work", primary: true, selected: true, writable: true }],
    });
    const header = container.querySelector(".calendar-week-header")!;
    expect(header).toHaveTextContent("Calendar · All calendar accounts");
    expect(header.querySelector(".eyebrow-account")).toHaveAttribute("title", "joel@example.com, work@example.com");
  });

  it("says all calendar accounts when the only connected account contributes", () => {
    const { container } = renderWeek();
    expect(container.querySelector(".calendar-week-header")).toHaveTextContent("Calendar · All calendar accounts");
  });

  it("names only the accounts contributing selected calendars when another connected account has none selected", () => {
    const { container } = renderWeek({
      accounts: [...accounts, { ...accounts[0], email: "work@example.com" }],
      calendars: [...calendars, { id: "work", accountId: "work@example.com", name: "Work", primary: true, selected: false, writable: true }],
    });
    expect(container.querySelector(".calendar-week-header")).toHaveTextContent("Calendar · joel@example.com");
    expect(container.querySelector(".calendar-week-header")).not.toHaveTextContent("All calendar accounts");
  });

  it("says none selected when no calendar is selected", () => {
    const { container } = renderWeek({ calendars: calendars.map((calendar) => ({ ...calendar, selected: false })) });
    const scope = container.querySelector(".calendar-week-header .eyebrow-account")!;
    expect(scope).toHaveTextContent("· None selected");
    expect(scope).not.toHaveAttribute("title");
  });

  it("requests the visible week, preloads neighbors, and refreshes when navigating", async () => {
    const listScheduleEvents = vi.mocked(mailClient.listScheduleEvents);
    renderWeek();
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(3));
    expect(new Date(listScheduleEvents.mock.calls[0][0])).toEqual(new Date(2026, 8, 20));
    expect(new Date(listScheduleEvents.mock.calls[0][1])).toEqual(new Date(2026, 8, 27));

    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(6));
    expect(listScheduleEvents).toHaveBeenCalledWith(new Date(2026, 8, 27).toISOString(), new Date(2026, 9, 4).toISOString(), expect.any(String));
    expect(listScheduleEvents).toHaveBeenCalledWith(new Date(2026, 8, 22).toISOString(), new Date(2026, 8, 23).toISOString(), expect.any(String));
    await screen.findByText("Sun 27");

    fireEvent.click(screen.getByRole("button", { name: "Previous week (-)" }));
    await screen.findByText("Sun 20");
  });

  it("navigates weeks with - and = and returns with Today", async () => {
    renderWeek();
    await screen.findByText("Sun 20");

    fireEvent.keyDown(window, { key: "=" });
    await screen.findByText("Sun 27");
    fireEvent.keyDown(window, { key: "=" });
    await screen.findByText("Sun 4");

    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    await screen.findByText("Sun 20");
  });

  it("uses the shared header buttons, ending with New Event like the other workspaces", async () => {
    const { container } = renderWeek();
    await screen.findByText("Sun 20");
    const controls = container.querySelector(".calendar-week-controls")!;

    expectSharedButtons(controls);
    const newEvent = screen.getByRole("button", { name: "New Event" });
    expect(newEvent).toHaveClass("btn");
    expect(newEvent.querySelector("svg")).not.toBeNull();
    expect(controls.lastElementChild).toBe(newEvent);
    expect(screen.getByRole("button", { name: "Today" })).toHaveClass("btn");
  });

  it("navigates weeks with Tab and Shift+Tab unless a control has focus", async () => {
    renderWeek();
    await screen.findByText("Sun 20");

    fireEvent.keyDown(window, { key: "Tab" });
    await screen.findByText("Sun 27");
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    await screen.findByText("Sun 20");
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    await screen.findByText("Sun 13");

    const today = screen.getByRole("button", { name: "Today" });
    today.focus();
    expect(fireEvent.keyDown(today, { key: "Tab" })).toBe(true);
    expect(screen.getByText("Sun 13")).toBeInTheDocument();
  });

  it("renders preloaded events while refreshing a newly displayed week", async () => {
    const nextStart = new Date(2026, 8, 27).toISOString();
    const preloaded = event("preloaded", "2026-09-28T09:00:00", "2026-09-28T10:00:00", "Cached planning");
    let nextCalls = 0;
    let resolve!: (value: { events: ScheduleEvent[]; errors: string[] }) => void;
    const refresh = new Promise<{ events: ScheduleEvent[]; errors: string[] }>((done) => { resolve = done; });
    vi.mocked(mailClient.listScheduleEvents).mockImplementation((start) => {
      if (start !== nextStart) return Promise.resolve({ events: [], errors: [] });
      nextCalls += 1;
      return nextCalls === 1 ? Promise.resolve({ events: [preloaded], errors: [] }) : refresh;
    });
    renderWeek();
    await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalledTimes(3));
    // Let the speculative request finish before navigating.
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    expect(calendarGrid().getByRole("button", { name: /Cached planning/ })).toBeInTheDocument();
    expect(screen.queryByText("Loading schedule…")).not.toBeInTheDocument();
    await act(async () => { resolve({ events: [], errors: [] }); });
    expect(calendarGrid().queryByRole("button", { name: /Cached planning/ })).not.toBeInTheDocument();
  });

  it("ignores week navigation keys while typing in a field", async () => {
    renderWeek();
    await screen.findByText("Sun 20");
    const input = document.createElement("input");
    document.body.append(input);
    input.focus();

    fireEvent.keyDown(input, { key: "=" });
    expect(screen.getByText("Sun 20")).toBeInTheDocument();
    input.remove();
  });

  it("places overlapping events side by side and opens details on click", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({
      events: [
        event("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Standup"),
        event("b", "2026-09-22T09:30:00", "2026-09-22T10:30:00", "Design review"),
      ],
      errors: [],
    });
    const { container } = renderWeek();

    const standup = await calendarGrid().findByRole("button", { name: /Standup/ });
    const review = calendarGrid().getByRole("button", { name: /Design review/ });
    expect(standup.style.width).toBe("50%");
    expect(standup.style.left).toBe("0%");
    expect(review.style.left).toBe("50%");
    expect(container.querySelectorAll(".calendar-week-column .calendar-schedule-event")).toHaveLength(2);

    fireEvent.click(standup);
    const viewer = await screen.findByRole("region", { name: "Standup details" });
    expect(within(viewer).getByText(/Tue, Sep 22/)).toBeInTheDocument();
  });

  it("opens a meeting's details when arriving from the conversation panel", async () => {
    const linked = event("linked", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Operations team meeting");
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [linked], errors: [] });
    const { container } = renderWeek({ initialEvent: linked });

    const viewer = await screen.findByRole("region", { name: "Operations team meeting details" });
    expect(within(viewer).getByText(/Tue, Sep 22/)).toBeInTheDocument();
    expect(container.querySelector(".calendar-week-scroll")?.scrollTop).toBeGreaterThan(0);
    const eventButton = await screen.findByRole("button", { name: "Operations team meeting" });
    expect(eventButton).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(eventButton);
    expect(screen.queryByRole("region", { name: "Operations team meeting details" })).not.toBeInTheDocument();
    fireEvent.click(eventButton);
    const reopened = await screen.findByRole("region", { name: "Operations team meeting details" });
    fireEvent.click(within(reopened).getByRole("button", { name: "Close Event Details" }));
    expect(screen.queryByRole("region", { name: "Operations team meeting details" })).not.toBeInTheDocument();
  });

  it("shows RSVP styling and changes one response through the event popup", async () => {
    const invited: ScheduleEvent = { ...event("primary:invited", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Team sync"), calendarId: "primary", responseStatus: "needsAction" as const, canRespond: true };
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [invited], errors: [] });
    const update = vi.spyOn(mailClient, "updateCalendarResponse").mockResolvedValue({ ...invited, responseStatus: "accepted" });
    renderWeek();
    const button = await screen.findByRole("button", { name: "Team sync" });
    expect(button).toHaveAttribute("data-response-status", "needsAction");
    fireEvent.click(button);
    const dialog = screen.getByRole("region", { name: "Team sync details" });
    // The question labels the answer buttons; no status line repeats what they show.
    expect(dialog).not.toHaveTextContent("Your response");
    expect(dialog).not.toHaveTextContent("Awaiting response");
    const going = within(dialog).getByRole("group", { name: "Going?" });
    for (const answer of within(going).getAllByRole("button")) expect(answer).toHaveAttribute("aria-pressed", "false");
    let finishRefetch!: (value: { events: typeof invited[]; errors: string[] }) => void;
    vi.mocked(mailClient.listScheduleEvents).mockImplementation(() => new Promise((resolve) => { finishRefetch = resolve; }));
    fireEvent.click(within(going).getByRole("button", { name: "Yes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(invited, "accepted"));
    await waitFor(() => expect(within(going).getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true"));
    // The grid keeps the week, with the new response, while the refetch is in flight.
    expect(screen.getByRole("button", { name: "Team sync" })).toHaveAttribute("data-response-status", "accepted");
    expect(screen.queryByText("Loading schedule…")).not.toBeInTheDocument();
    await act(async () => { finishRefetch({ events: [{ ...invited, responseStatus: "accepted" }], errors: [] }); });
    expect(screen.getByRole("button", { name: "Team sync" })).toHaveAttribute("data-response-status", "accepted");
  });

  it("keeps the prior RSVP and reports a failed change", async () => {
    const invited = { ...event("primary:invited", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Review"), calendarId: "primary", responseStatus: "tentative" as const, canRespond: true };
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [invited], errors: [] });
    vi.spyOn(mailClient, "updateCalendarResponse").mockRejectedValue(new Error("Calendar unavailable"));
    renderWeek();
    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = screen.getByRole("region", { name: "Review details" });
    fireEvent.click(within(dialog).getByRole("button", { name: "No" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Calendar unavailable");
    expect(within(dialog).getByRole("button", { name: "Maybe" })).toHaveAttribute("aria-pressed", "true");
  });

  describe("owned event actions", () => {
    const owned = (overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
      ...event("primary:owned", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Planning"),
      calendarId: "primary",
      canEdit: true,
      ...overrides,
    });

    it("offers no edit or delete for events the user does not organize", async () => {
      const invited = { ...owned({ canEdit: false, canRespond: true, responseStatus: "accepted" as const }) };
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [invited], errors: [] });
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      const dialog = screen.getByRole("region", { name: "Planning details" });
      expect(within(dialog).queryByRole("button", { name: "Edit Event" })).not.toBeInTheDocument();
      expect(within(dialog).queryByRole("button", { name: "Delete Event" })).not.toBeInTheDocument();
    });

    it("deletes an owned event only after confirmation and drops it from the grid", async () => {
      const planning = owned({ attendees: ["jane@example.com"] });
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [planning], errors: [] });
      const remove = vi.spyOn(mailClient, "deleteCalendarEvent").mockResolvedValue();
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      const dialog = screen.getByRole("region", { name: "Planning details" });
      fireEvent.click(within(dialog).getByRole("button", { name: "Delete Event" }));
      const confirm = within(dialog).getByRole("group", { name: "Delete event confirmation" });
      expect(confirm).toHaveTextContent("Delete this event? Guests will be notified.");
      fireEvent.click(within(confirm).getByRole("button", { name: "Keep" }));
      expect(within(dialog).queryByRole("group", { name: "Delete event confirmation" })).not.toBeInTheDocument();
      expect(remove).not.toHaveBeenCalled();

      fireEvent.click(within(dialog).getByRole("button", { name: "Delete Event" }));
      vi.mocked(mailClient.listScheduleEvents).mockImplementation(() => new Promise(() => {}));
      fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
      await waitFor(() => expect(remove).toHaveBeenCalledWith(planning));
      // The deleted event leaves the grid before the refetch finishes.
      await waitFor(() => expect(screen.queryByRole("region", { name: "Planning details" })).not.toBeInTheDocument());
      expect(calendarGrid().queryByRole("button", { name: /^Planning/ })).not.toBeInTheDocument();
    });

    it("keeps the event and reports a failed delete", async () => {
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [owned()], errors: [] });
      vi.spyOn(mailClient, "deleteCalendarEvent").mockRejectedValue(new Error("Only events you organize can be changed"));
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      const dialog = screen.getByRole("region", { name: "Planning details" });
      fireEvent.click(within(dialog).getByRole("button", { name: "Delete Event" }));
      expect(within(dialog).getByRole("group", { name: "Delete event confirmation" })).not.toHaveTextContent("Guests will be notified");
      fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
      expect(await within(dialog).findByRole("alert")).toHaveTextContent("Only events you organize can be changed");
      expect(calendarGrid().getByRole("button", { name: /^Planning/ })).toBeInTheDocument();
    });

    it("edits an owned timed event and shows the change in the grid and popup", async () => {
      const planning = owned({ location: "Room 4B", attendees: ["jane@example.com"], description: "Agenda" });
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [planning], errors: [] });
      const update = vi.spyOn(mailClient, "updateCalendarEvent").mockImplementation(async (request) => ({
        ...planning, title: request.title, start: request.start, end: request.end, location: request.location, attendees: request.attendees,
      }));
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      fireEvent.click(within(screen.getByRole("region", { name: "Planning details" })).getByRole("button", { name: "Edit Event" }));
      const form = screen.getByRole("dialog", { name: "Edit event" });
      expect(within(form).getByLabelText("Title")).toHaveValue("Planning");
      expect(within(form).getByLabelText("Starts")).toHaveValue("2026-09-22T09:00");
      expect(within(form).getByLabelText("Ends")).toHaveValue("2026-09-22T10:00");
      expect(within(form).getByLabelText("Location")).toHaveValue("Room 4B");
      expect(within(form).getByRole("button", { name: "Remove jane@example.com" })).toBeInTheDocument();
      expect(within(form).getByLabelText("All day")).not.toBeChecked();
      fireEvent.change(within(form).getByLabelText("Title"), { target: { value: "Quarterly planning" } });
      fireEvent.change(within(form).getByLabelText("Ends"), { target: { value: "2026-09-22T11:00" } });
      const invitees = within(form).getByRole("textbox", { name: "Invite People" });
      fireEvent.change(invitees, { target: { value: "Theo@Example.com" } });
      fireEvent.blur(invitees);
      expect(within(form).getByRole("button", { name: "Remove Theo@Example.com" })).toBeInTheDocument();
      vi.mocked(mailClient.listScheduleEvents).mockImplementation(() => new Promise(() => {}));
      fireEvent.click(within(form).getByRole("button", { name: "Save changes" }));
      await waitFor(() => expect(update).toHaveBeenCalledWith({
        accountId: "joel@example.com",
        calendarId: "primary",
        eventId: "primary:owned",
        title: "Quarterly planning",
        start: new Date(2026, 8, 22, 9).toISOString(),
        end: new Date(2026, 8, 22, 11).toISOString(),
        allDay: false,
        location: "Room 4B",
        description: "Agenda",
        attendees: ["jane@example.com", "theo@example.com"],
      }));
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit event" })).not.toBeInTheDocument());
      expect(screen.getByRole("region", { name: "Quarterly planning details" })).toBeInTheDocument();
      expect(calendarGrid().getByRole("button", { name: /^Quarterly planning/ })).toBeInTheDocument();
    });

    it("edits all-day dates inclusively and returns an untouched HTML description verbatim", async () => {
      const offsite = owned({ id: "primary:offsite", title: "Offsite", allDay: true, start: "2026-09-23", end: "2026-09-25", description: "Bring <b>laptops</b>" });
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [offsite], errors: [] });
      const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(offsite);
      renderWeek();
      const offsiteDays = await calendarGrid().findAllByRole("button", { name: "Offsite" });
      expect(offsiteDays).toHaveLength(2);
      fireEvent.click(offsiteDays[0]);
      fireEvent.click(within(screen.getByRole("region", { name: "Offsite details" })).getByRole("button", { name: "Edit Event" }));
      const form = screen.getByRole("dialog", { name: "Edit event" });
      expect(within(form).getByLabelText("All day")).toBeChecked();
      expect(within(form).getByLabelText("Starts")).toHaveValue("2026-09-23");
      // Google's end date is exclusive; the form shows the last day.
      expect(within(form).getByLabelText("Ends")).toHaveValue("2026-09-24");
      expect(within(form).getByLabelText("Description")).toHaveValue("Bring laptops");
      fireEvent.change(within(form).getByLabelText("Ends"), { target: { value: "2026-09-25" } });
      fireEvent.click(within(form).getByRole("button", { name: "Save changes" }));
      await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({
        allDay: true, start: "2026-09-23", end: "2026-09-26", description: "Bring <b>laptops</b>",
      })));
    });

    it("converts between all-day and timed and rejects an end before the start", async () => {
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [owned()], errors: [] });
      const update = vi.spyOn(mailClient, "updateCalendarEvent").mockImplementation(async () => owned());
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      fireEvent.click(within(screen.getByRole("region", { name: "Planning details" })).getByRole("button", { name: "Edit Event" }));
      const form = screen.getByRole("dialog", { name: "Edit event" });
      fireEvent.change(within(form).getByLabelText("Ends"), { target: { value: "2026-09-22T08:00" } });
      fireEvent.click(within(form).getByRole("button", { name: "Save changes" }));
      expect(await within(form).findByRole("alert")).toHaveTextContent("The end time must be after the start time.");
      expect(update).not.toHaveBeenCalled();

      fireEvent.click(within(form).getByLabelText("All day"));
      expect(within(form).getByLabelText("Starts")).toHaveValue("2026-09-22");
      expect(within(form).getByLabelText("Ends")).toHaveValue("2026-09-22");
      fireEvent.click(within(form).getByRole("button", { name: "Save changes" }));
      await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ allDay: true, start: "2026-09-22", end: "2026-09-23" })));
    });

    it("keeps the edit dialog open with the reason when saving fails", async () => {
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [owned()], errors: [] });
      vi.spyOn(mailClient, "updateCalendarEvent").mockRejectedValue(new Error("Calendar write access was denied."));
      renderWeek();
      fireEvent.click(await calendarGrid().findByRole("button", { name: /^Planning/ }));
      fireEvent.click(within(screen.getByRole("region", { name: "Planning details" })).getByRole("button", { name: "Edit Event" }));
      const form = screen.getByRole("dialog", { name: "Edit event" });
      fireEvent.click(within(form).getByRole("button", { name: "Save changes" }));
      expect(await within(form).findByRole("alert")).toHaveTextContent("Calendar write access was denied.");
      // Pointer presses inside the portaled dialog must not dismiss the event popup underneath.
      fireEvent.pointerDown(within(form).getByLabelText("Title"));
      fireEvent.click(within(form).getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog", { name: "Edit event" })).not.toBeInTheDocument();
      expect(screen.getByRole("region", { name: "Planning details" })).toBeInTheDocument();
    });
  });

  it("uses free overlap space and keeps short meeting titles visible", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({
      events: [
        event("long", "2026-09-22T09:00:00", "2026-09-22T11:00:00", "Long meeting"),
        event("early", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Early review"),
        event("middle", "2026-09-22T09:30:00", "2026-09-22T10:30:00", "Middle meeting"),
        event("third", "2026-09-22T10:00:00", "2026-09-22T10:15:00", "Third meeting"),
      ],
      errors: [],
    });
    renderWeek();

    const early = await screen.findByRole("button", { name: "Early review" });
    const middle = calendarGrid().getByRole("button", { name: /Middle meeting/ });
    expect(parseFloat(early.style.left)).toBeCloseTo(100 / 3);
    expect(parseFloat(early.style.width)).toBeCloseTo(200 / 3);
    expect(parseFloat(middle.style.width)).toBeCloseTo(100 / 3);
    expect(early.querySelector("strong")).toHaveTextContent("Early review");
    expect(early.querySelector("span")).toBeNull();
    expect(early.title).toContain("9:00");
  });

  it("shows event details in the shared right context rail", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({
      events: [event("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Standup")],
      errors: [],
    });
    const { container } = renderWeek();

    fireEvent.click(await calendarGrid().findByRole("button", { name: /Standup/ }));
    const viewer = await screen.findByRole("region", { name: "Standup details" });

    // Scoped to the grid column so it never overlaps the month/calendar rail,
    // and width-capped rather than pinned to both edges.
    expect(container.querySelector(".workspace-context-rail")).toContainElement(viewer);
    expect(container.querySelector(".calendar-week-side")).not.toContainElement(viewer);
  });

  it("renders all-day events in their own row above the time grid", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({
      events: [{
        id: "holiday", accountId: "joel@example.com", title: "Labor Day",
        start: "2026-09-21", end: "2026-09-22", allDay: true,
      }],
      errors: [],
    });
    renderWeek();

    const allDay = await screen.findByLabelText("All-day events");
    expect(within(allDay).getByRole("button", { name: "Labor Day" })).toBeInTheDocument();
  });

  it("marks the current time only on today's column", async () => {
    const { container } = renderWeek();
    await screen.findByText("Sun 20");
    const indicators = container.querySelectorAll("[data-testid='calendar-now-indicator']");
    expect(indicators).toHaveLength(1);
    // 14:30 with a 64px hour row.
    expect((indicators[0] as HTMLElement).style.top).toBe(`${14.5 * 64}px`);

    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    await screen.findByText("Sun 27");
    expect(container.querySelectorAll("[data-testid='calendar-now-indicator']")).toHaveLength(0);
  });

  it("jumps to the week of a day picked in the mini month", async () => {
    renderWeek();
    await screen.findByText("Sun 20");

    fireEvent.click(screen.getByRole("gridcell", { name: /September 8, 2026/ }));
    await screen.findByText("Sun 6");
  });

  it("pages the mini month without changing the displayed week", async () => {
    renderWeek();
    await screen.findByText("Sun 20");

    fireEvent.click(screen.getByRole("button", { name: "Next Month" }));
    expect(screen.getByRole("heading", { level: 3, name: "October 2026" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "September 2026" })).toBeInTheDocument();
    expect(screen.getByText("Sun 20")).toBeInTheDocument();
  });

  it("covers the whole month plus leading and trailing days in six rows", () => {
    const days = monthGridDays(new Date(2026, 8, 22));
    expect(days).toHaveLength(42);
    expect(days[0].getDay()).toBe(0);
    expect(days[0]).toEqual(new Date(2026, 7, 30));
    expect(days.filter((day) => day.getMonth() === 8)).toHaveLength(30);
  });

  it("lists connected calendars and reports selection changes", async () => {
    const { props } = renderWeek();
    await screen.findByText("Sun 20");

    const list = screen.getByRole("region", { name: "Calendars" });
    expect(within(list).getByText("joel@example.com", { selector: ".calendar-list-account-toggle span" })).toBeInTheDocument();
    const holidays = within(list).getByRole("checkbox", { name: "Holidays in United States" });
    expect(holidays).toBeChecked();

    fireEvent.click(holidays);
    expect(props.onToggleCalendar).toHaveBeenCalledWith("joel@example.com", "holidays", false);
  });

  it("remembers which calendar accounts are collapsed across visits", async () => {
    const twoAccounts = {
      accounts: [...accounts, { ...accounts[0], email: "work@example.com" }],
      calendars: [...calendars, { id: "work", accountId: "work@example.com", name: "Work", primary: true, selected: true, writable: true }],
    };
    const toggle = (email: string) => within(screen.getByRole("region", { name: "Calendars" })).getByRole("button", { name: email });
    const { unmount } = renderWeek(twoAccounts);
    await screen.findByText("Sun 20");
    expect(toggle("joel@example.com")).toHaveAttribute("aria-expanded", "true");

    fireEvent.click(toggle("joel@example.com"));
    expect(toggle("joel@example.com")).toHaveAttribute("aria-expanded", "false");
    expect(JSON.parse(localStorage.getItem(COLLAPSED_CALENDAR_ACCOUNTS_KEY)!)).toEqual(["joel@example.com"]);
    unmount();

    renderWeek(twoAccounts);
    await screen.findByText("Sun 20");
    expect(toggle("joel@example.com")).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("checkbox", { name: "Holidays in United States" })).not.toBeInTheDocument();
    expect(toggle("work@example.com")).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("checkbox", { name: "Work" })).toBeInTheDocument();

    fireEvent.click(toggle("joel@example.com"));
    expect(screen.getByRole("checkbox", { name: "Holidays in United States" })).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem(COLLAPSED_CALENDAR_ACCOUNTS_KEY)!)).toEqual([]);
  });

  it("expands every calendar account when the remembered state is unreadable", async () => {
    localStorage.setItem(COLLAPSED_CALENDAR_ACCOUNTS_KEY, "{not json");
    renderWeek();
    await screen.findByText("Sun 20");
    expect(screen.getByRole("checkbox", { name: "Holidays in United States" })).toBeInTheDocument();

    cleanup();
    localStorage.setItem(COLLAPSED_CALENDAR_ACCOUNTS_KEY, JSON.stringify({ "joel@example.com": true }));
    renderWeek();
    await screen.findByText("Sun 20");
    expect(screen.getByRole("checkbox", { name: "Holidays in United States" })).toBeInTheDocument();
  });

  describe("open tasks", () => {
    function task(id: string, overrides: Partial<ThreadTask> = {}): ThreadTask {
      return {
        id, accountId: "joel@example.com", threadId: null, subjectSnapshot: null, title: id, kind: "action",
        dueKind: "none", status: "open", createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z", ...overrides,
      };
    }

    const proposal = task("proposal", { title: "Write proposal" });

    function openTasks() {
      return screen.getByRole("region", { name: "Open tasks" });
    }

    /** Drags the row onto the week, releasing over `target` (null: outside the grid). */
    function dragTask(row: HTMLElement, target: HTMLElement | null, clientY: number) {
      document.elementFromPoint = vi.fn(() => target);
      fireEvent.pointerDown(row, { button: 0, pointerId: 7, clientX: 900, clientY: 10 });
      fireEvent.pointerMove(document.body, { pointerId: 7, clientX: 50, clientY });
    }

    function tuesdayColumn(container: HTMLElement) {
      const column = container.querySelectorAll<HTMLElement>(".calendar-week-column")[2]!;
      vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);
      return column;
    }

    afterEach(() => {
      Reflect.deleteProperty(document, "elementFromPoint");
    });

    it("lists the To Do and In Progress tasks of the shown accounts in board order and says which have time booked", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([
        task("someday", { title: "Clean up notes", createdAt: "2026-09-02T00:00:00Z" }),
        task("doing", { title: "Draft budget", status: "in_progress", createdAt: "2026-09-01T00:00:00Z" }),
        task("due", { title: "Send invoice", dueKind: "date", dueValue: "2026-09-21" }),
        task("done", { title: "Finished", status: "completed" }),
        task("dropped", { title: "Dropped", status: "cancelled" }),
        task("elsewhere", { title: "Other account", accountId: "other@example.com" }),
      ]);
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [
        { ...event("block", "2026-09-23T10:00:00", "2026-09-23T10:30:00", "Draft budget"), sourceTaskId: "doing" },
      ], errors: [] });
      renderWeek({ mailAccountIds: ["joel@example.com"] });

      const section = openTasks();
      await within(section).findByRole("button", { name: /Send invoice/ });
      expectContextRows(section);
      expect([...section.querySelectorAll(".context-row-title")].map((row) => row.textContent)).toEqual(["Send invoice", "Draft budget", "Clean up notes"]);
      expect(within(section).getByRole("button", { name: /Send invoice/ }).closest(".context-row")).toHaveTextContent("Due");
      await waitFor(() => expect(within(section).getByRole("button", { name: /Draft budget/ })).toHaveTextContent("In progress · Booked Wed 10:00 am"));
      expect(mailClient.listTasks).toHaveBeenCalledWith("joel@example.com");
      // The card sits below the agenda in the same rail.
      const agenda = screen.getByRole("region", { name: "Today's agenda" });
      expect(agenda.compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    });

    it("says when there are no open tasks and retries a failed load", async () => {
      vi.mocked(mailClient.listTasks).mockRejectedValueOnce(new Error("offline"));
      renderWeek({ mailAccountIds: ["joel@example.com"] });
      fireEvent.click(await within(openTasks()).findByRole("button", { name: "Retry tasks" }));
      expect(await within(openTasks()).findByText("No open tasks.")).toBeInTheDocument();
    });

    it("books thirty minutes where a task is dropped, tagged with the task, and Undo removes it", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const created = { ...event("block", "2026-09-22T09:15:00", "2026-09-22T09:45:00", "Write proposal"), calendarId: "primary", sourceTaskId: "proposal" };
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(created);
      const remove = vi.spyOn(mailClient, "deleteCalendarEvent").mockResolvedValue();
      const onNotice = vi.fn();
      const onCreated = vi.fn();
      const { container } = renderWeek({ mailAccountIds: ["joel@example.com"], onNotice, onCreated });
      const column = tuesdayColumn(container);
      const row = (await within(openTasks()).findByRole("button", { name: /Write proposal/ })).closest<HTMLElement>(".context-row")!;

      dragTask(row, column, 9.25 * 64);
      expect(row).toHaveClass("calendar-task-dragging");
      const preview = container.querySelector(".calendar-create-selection");
      expect(preview).toHaveStyle({ top: `${9.25 * 64}px`, height: `${0.5 * 64}px` });
      expect(preview).toHaveTextContent("Write proposal");
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 9.25 * 64 });

      await waitFor(() => expect(create).toHaveBeenCalledWith({
        accountId: "joel@example.com", calendarId: "primary", title: "Write proposal",
        start: new Date(2026, 8, 22, 9, 15).toISOString(), end: new Date(2026, 8, 22, 9, 45).toISOString(),
        description: "", attendees: [], sourceTaskId: "proposal",
      }));
      expect(container.querySelector(".calendar-create-selection")).not.toBeInTheDocument();
      expect(row).not.toHaveClass("calendar-task-dragging");
      // Dropping books at once; it never opens New Event.
      expect(screen.queryByRole("dialog", { name: "New event" })).not.toBeInTheDocument();
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: expect.any(String), undo: expect.any(Function) }));
      // ICU versions differ on the space before "am" (plain or narrow no-break).
      expect(onNotice.mock.calls[0]![0].message.replace(/\s/gu, " ")).toBe("Booked Tue 9:15–9:45 am for “Write proposal”");
      expect(onCreated).toHaveBeenCalledOnce();

      act(() => onNotice.mock.calls[0]![0].undo());
      await waitFor(() => expect(remove).toHaveBeenCalledWith(created));
      await waitFor(() => expect(onNotice).toHaveBeenLastCalledWith({ message: "Booked time removed" }));
    });

    it("books on the task's own account, showing a hidden calendar first", async () => {
      const otherAccounts: CalendarAccount[] = [...accounts, { email: "work@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "connected" }];
      const otherCalendars: CalendarOption[] = [...calendars, { id: "work-primary", accountId: "work@example.com", name: "work@example.com", primary: true, selected: false, writable: true }];
      vi.mocked(mailClient.listTasks).mockResolvedValue([{ ...proposal, accountId: "work@example.com" }]);
      const select = vi.spyOn(mailClient, "setCalendarSelection").mockResolvedValue([]);
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("block", "2026-09-22T13:00:00", "2026-09-22T13:30:00"));
      const { container } = renderWeek({ accounts: otherAccounts, calendars: otherCalendars, mailAccountIds: ["joel@example.com", "work@example.com"], onNotice: vi.fn() });
      const column = tuesdayColumn(container);
      const row = (await within(openTasks()).findByRole("button", { name: /Write proposal/ })).closest<HTMLElement>(".context-row")!;
      expect(row).toHaveTextContent("work@example.com");

      dragTask(row, column, 13 * 64);
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 13 * 64 });
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ accountId: "work@example.com", calendarId: "work-primary" })));
      expect(select).toHaveBeenCalledWith("work@example.com", ["work-primary"]);
      expect(select.mock.invocationCallOrder[0]).toBeLessThan(create.mock.invocationCallOrder[0]!);
    });

    it("keeps a dropped block inside its day", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("block", "2026-09-22T23:30:00", "2026-09-23T00:00:00"));
      const { container } = renderWeek({ mailAccountIds: ["joel@example.com"], onNotice: vi.fn() });
      const column = tuesdayColumn(container);
      const row = (await within(openTasks()).findByRole("button", { name: /Write proposal/ })).closest<HTMLElement>(".context-row")!;
      dragTask(row, column, 23.9 * 64);
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 23.9 * 64 });
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
        start: new Date(2026, 8, 22, 23, 30).toISOString(), end: new Date(2026, 8, 23, 0, 0).toISOString(),
      })));
    });

    it("books nothing when a drag ends outside the week, is cancelled with Escape, or never moves", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const create = vi.spyOn(mailClient, "createCalendarEvent");
      const { container } = renderWeek({ mailAccountIds: ["joel@example.com"], onNotice: vi.fn() });
      const column = tuesdayColumn(container);
      const row = (await within(openTasks()).findByRole("button", { name: /Write proposal/ })).closest<HTMLElement>(".context-row")!;

      dragTask(row, null, 9 * 64);
      expect(container.querySelector(".calendar-create-selection")).not.toBeInTheDocument();
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 9 * 64 });

      dragTask(row, column, 9 * 64);
      expect(container.querySelector(".calendar-create-selection")).toBeInTheDocument();
      const keydown = vi.fn();
      window.addEventListener("keydown", keydown);
      fireEvent.keyDown(document.body, { key: "Escape" });
      window.removeEventListener("keydown", keydown);
      // Escape ends only the drag; it doesn't reach the app's own Escape handling.
      expect(keydown).not.toHaveBeenCalled();
      expect(container.querySelector(".calendar-create-selection")).not.toBeInTheDocument();
      expect(row).not.toHaveClass("calendar-task-dragging");
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 9 * 64 });

      // A press that stays put is a click, handled by the row's button.
      document.elementFromPoint = vi.fn(() => column);
      fireEvent.pointerDown(row, { button: 0, pointerId: 8, clientX: 900, clientY: 10 });
      fireEvent.pointerMove(document.body, { pointerId: 8, clientX: 902, clientY: 11 });
      fireEvent.pointerUp(document.body, { pointerId: 8, clientX: 902, clientY: 11 });
      await act(async () => {});
      expect(create).not.toHaveBeenCalled();
    });

    it("opens New Event for a clicked task at the coming half hour, tagged with the task", async () => {
      vi.setSystemTime(new Date(2026, 8, 22, 14, 40));
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("block", "2026-09-22T15:00:00", "2026-09-22T15:30:00", "Write proposal"));
      renderWeek({ mailAccountIds: ["joel@example.com"] });
      fireEvent.click(await within(openTasks()).findByRole("button", { name: /Write proposal/ }));

      const dialog = await screen.findByRole("dialog", { name: "New event" });
      expect(within(dialog).getByLabelText("Title")).toHaveValue("Write proposal");
      expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-09-22T15:00");
      expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-09-22T15:30");
      expect(within(dialog).getByLabelText("Calendar")).toHaveValue("joel@example.com\nprimary");
      fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
      await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ title: "Write proposal", sourceTaskId: "proposal" })));
    });

    it("explains when no calendar can take the booked time", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const create = vi.spyOn(mailClient, "createCalendarEvent");
      const onNotice = vi.fn();
      const { container } = renderWeek({ calendars: calendars.map((calendar) => ({ ...calendar, writable: false })), mailAccountIds: ["joel@example.com"], onNotice });
      const column = tuesdayColumn(container);
      const row = (await within(openTasks()).findByRole("button", { name: /Write proposal/ })).closest<HTMLElement>(".context-row")!;
      dragTask(row, column, 9 * 64);
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 9 * 64 });
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "Connect a Google Calendar account with event access to book time for tasks." }));
      expect(create).not.toHaveBeenCalled();
    });

    it("keeps a checked-off task a moment, struck through, then drops it", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      const budget = task("budget", { title: "Draft budget" });
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal, budget]);
      const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue({ ...proposal, status: "completed" });
      const onTasksChanged = vi.fn();
      const onNotice = vi.fn();
      renderWeek({ mailAccountIds: ["joel@example.com"], onTasksChanged, onNotice });
      const box = await within(openTasks()).findByRole("checkbox", { name: "Mark Write proposal done" });

      // The reload after completing no longer lists it; the row stays in place regardless.
      vi.mocked(mailClient.listTasks).mockResolvedValue([budget]);
      fireEvent.click(box);
      await waitFor(() => expect(setStatus).toHaveBeenCalledWith("proposal", "completed"));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "Completed: Write proposal", undo: expect.any(Function) }));
      expect(onTasksChanged).toHaveBeenCalledOnce();
      await waitFor(() => expect(mailClient.listTasks).toHaveBeenCalledTimes(2));
      const done = within(openTasks()).getByRole("checkbox", { name: "Mark Write proposal not done" });
      expect(done).toBeChecked();
      expect(done.closest(".context-row")).toHaveClass("context-task-done");
      expect([...openTasks().querySelectorAll(".context-row-title")].map((row) => row.textContent)).toEqual(["Write proposal", "Draft budget"]);
      // A finished task can no longer be booked by clicking it.
      expect(within(openTasks()).queryByRole("button", { name: /Write proposal/ })).not.toBeInTheDocument();

      act(() => { vi.advanceTimersByTime(DONE_TASK_LINGER_MS); });
      expect(within(openTasks()).queryByText("Write proposal")).not.toBeInTheDocument();
      expect(within(openTasks()).getByRole("checkbox", { name: "Mark Draft budget done" })).toBeInTheDocument();
    });

    it("unchecking a just-finished task puts back the status it had", async () => {
      const doing = task("doing", { title: "Draft budget", status: "in_progress" });
      vi.mocked(mailClient.listTasks).mockResolvedValue([doing]);
      const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (id, status) => ({ ...doing, status }));
      const onTasksChanged = vi.fn();
      renderWeek({ mailAccountIds: ["joel@example.com"], onTasksChanged, onNotice: vi.fn() });

      vi.mocked(mailClient.listTasks).mockResolvedValue([]);
      fireEvent.click(await within(openTasks()).findByRole("checkbox", { name: "Mark Draft budget done" }));
      vi.mocked(mailClient.listTasks).mockResolvedValue([doing]);
      fireEvent.click(await within(openTasks()).findByRole("checkbox", { name: "Mark Draft budget not done" }));
      await waitFor(() => expect(setStatus).toHaveBeenLastCalledWith("doing", "in_progress"));
      await waitFor(() => expect(onTasksChanged).toHaveBeenCalledTimes(2));
      const row = await within(openTasks()).findByRole("button", { name: /Draft budget/ });
      expect(row).toHaveTextContent("In progress");
      expect(row.closest(".context-row")).not.toHaveClass("context-task-done");
    });

    it("Undo in the notice brings back a task that has already left the list", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (id, status) => ({ ...proposal, status }));
      const onNotice = vi.fn();
      renderWeek({ mailAccountIds: ["joel@example.com"], onNotice });

      vi.mocked(mailClient.listTasks).mockResolvedValue([]);
      fireEvent.click(await within(openTasks()).findByRole("checkbox", { name: "Mark Write proposal done" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith(expect.objectContaining({ message: "Completed: Write proposal" })));
      act(() => { vi.advanceTimersByTime(DONE_TASK_LINGER_MS); });
      expect(await within(openTasks()).findByText("No open tasks.")).toBeInTheDocument();

      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      act(() => onNotice.mock.calls[0]![0].undo());
      expect(await within(openTasks()).findByRole("checkbox", { name: "Mark Write proposal done" })).not.toBeChecked();
      await waitFor(() => expect(setStatus).toHaveBeenLastCalledWith("proposal", "open"));
    });

    it("says why a task couldn't be completed and leaves it open", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      vi.spyOn(mailClient, "setTaskStatus").mockRejectedValue(new Error("offline"));
      const onNotice = vi.fn();
      renderWeek({ mailAccountIds: ["joel@example.com"], onNotice });
      fireEvent.click(await within(openTasks()).findByRole("checkbox", { name: "Mark Write proposal done" }));
      await waitFor(() => expect(onNotice).toHaveBeenCalledWith({ message: "offline" }));
      expect(within(openTasks()).getByRole("checkbox", { name: "Mark Write proposal done" })).not.toBeChecked();
    });

    it("a press on the checkbox never starts a drag", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      const { container } = renderWeek({ mailAccountIds: ["joel@example.com"], onNotice: vi.fn() });
      const column = tuesdayColumn(container);
      const box = await within(openTasks()).findByRole("checkbox", { name: "Mark Write proposal done" });
      dragTask(box, column, 9 * 64);
      expect(box.closest(".context-row")).not.toHaveClass("calendar-task-dragging");
      expect(container.querySelector(".calendar-create-selection")).not.toBeInTheDocument();
      fireEvent.pointerUp(document.body, { pointerId: 7, clientX: 50, clientY: 9 * 64 });
    });

    it("names the booked task in event details, where it can be marked done", async () => {
      vi.mocked(mailClient.listTasks).mockResolvedValue([proposal]);
      vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [
        { ...event("block", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Write proposal"), calendarId: "primary", sourceTaskId: "proposal" },
      ], errors: [] });
      const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockResolvedValue({ ...proposal, status: "completed" });
      const onTasksChanged = vi.fn();
      renderWeek({ mailAccountIds: ["joel@example.com"], onTasksChanged });
      await within(openTasks()).findByRole("button", { name: /Write proposal/ });
      fireEvent.click(await calendarGrid().findByRole("button", { name: "Write proposal" }));

      const booked = await screen.findByRole("region", { name: "Task" });
      fireEvent.click(within(booked).getByRole("checkbox", { name: "Mark Write proposal done" }));
      await waitFor(() => expect(setStatus).toHaveBeenCalledWith("proposal", "completed"));
      expect(onTasksChanged).toHaveBeenCalledOnce();
    });
  });

  it("opens an hour-long event from a clicked time and saves its details on the primary calendar", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("created", "2026-09-22T09:15:00", "2026-09-22T10:15:00", "Planning"));
    const onCreated = vi.fn();
    const { container } = renderWeek({ onCreated });
    const column = container.querySelectorAll<HTMLElement>(".calendar-week-column")[2]!;
    vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);

    fireEvent.pointerDown(column, { button: 0, pointerId: 1, clientY: 9.25 * 64 });
    fireEvent.pointerUp(column, { button: 0, pointerId: 1, clientY: 9.25 * 64 });
    const dialog = await screen.findByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-09-22T09:15");
    expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-09-22T10:15");
    expect(within(dialog).getByLabelText("Calendar")).toHaveValue("joel@example.com\nprimary");
    fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Planning" } });
    // A pasted or typed list becomes one badge per address when the field commits.
    const invitees = within(dialog).getByRole("textbox", { name: "Invite People" });
    fireEvent.change(invitees, { target: { value: "Ada@Example.com, bob@example.com" } });
    fireEvent.blur(invitees);
    fireEvent.change(within(dialog).getByLabelText("Description"), { target: { value: "Review the plan" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));

    await waitFor(() => expect(create).toHaveBeenCalledWith({
      accountId: "joel@example.com", calendarId: "primary", title: "Planning",
      start: new Date(2026, 8, 22, 9, 15).toISOString(),
      end: new Date(2026, 8, 22, 10, 15).toISOString(),
      allDay: false, location: "",
      attendees: ["ada@example.com", "bob@example.com"], description: "Review the plan",
    }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New event" })).not.toBeInTheDocument());
    expect(await screen.findByRole("region", { name: "Planning details" })).toBeInTheDocument();
    expect(onCreated).toHaveBeenCalledOnce();
  });

  it("uses the dragged span and allows changing the calendar", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("created", "2026-09-22T09:00:00", "2026-09-22T11:30:00"));
    const select = vi.spyOn(mailClient, "setCalendarSelection").mockResolvedValue([]);
    const { container } = renderWeek({
      calendars: [...calendars, { id: "team", accountId: "joel@example.com", name: "Team", primary: false, selected: false, writable: true }],
    });
    const column = container.querySelectorAll<HTMLElement>(".calendar-week-column")[2]!;
    vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);
    fireEvent.pointerDown(column, { button: 0, pointerId: 2, clientY: 9 * 64 });
    fireEvent.pointerMove(column, { pointerId: 2, clientY: 11.25 * 64 });
    expect(container.querySelector(".calendar-create-selection")).toHaveStyle({ top: `${9 * 64}px`, height: `${2.5 * 64}px` });
    fireEvent.pointerUp(column, { button: 0, pointerId: 2, clientY: 11.25 * 64 });

    const dialog = await screen.findByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-09-22T11:30");
    fireEvent.change(within(dialog).getByLabelText("Calendar"), { target: { value: "joel@example.com\nteam" } });
    fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Team sync" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ calendarId: "team", title: "Team sync" })));
    expect(select).toHaveBeenCalledWith("joel@example.com", ["primary", "holidays", "team"]);
  });

  it("keeps a clicked late-night event one hour long across midnight", async () => {
    const { container } = renderWeek();
    const column = container.querySelectorAll<HTMLElement>(".calendar-week-column")[2]!;
    vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);
    fireEvent.pointerDown(column, { button: 0, pointerId: 3, clientY: 23.75 * 64 });
    fireEvent.pointerUp(column, { button: 0, pointerId: 3, clientY: 23.75 * 64 });
    const dialog = await screen.findByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-09-22T23:45");
    expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-09-23T00:45");
  });

  it("keeps the dialog open on a save error and excludes read-only calendars", async () => {
    vi.spyOn(mailClient, "createCalendarEvent").mockRejectedValue(new Error("Reconnect this calendar account"));
    renderWeek();
    fireEvent.click(screen.getByRole("button", { name: "New Event" }));
    const dialog = await screen.findByRole("dialog", { name: "New event" });
    expect(within(dialog).getByRole("option", { name: /joel@example.com/ })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: /Holidays/ })).not.toBeInTheDocument();
    fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Planning" } });
    fireEvent.change(within(dialog).getByLabelText("Ends"), { target: { value: "2026-09-22T08:00" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("end time must be after");
    expect(mailClient.createCalendarEvent).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Ends"), { target: { value: "2026-09-22T16:00" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Reconnect this calendar account");
    expect(dialog).toBeInTheDocument();
  });

  it("colors one calendar's meetings from the calendar's options menu", async () => {
    const standup = { ...event("primary:standup", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Standup"), calendarId: "primary" };
    const parade = { ...event("holidays:parade", "2026-09-23T09:00:00", "2026-09-23T10:00:00", "Parade"), calendarId: "holidays" };
    const allDay = { ...event("primary:offsite", "2026-09-24", "2026-09-25", "Offsite"), calendarId: "primary", allDay: true };
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [standup, parade, allDay], errors: [] });
    renderWeek();
    const meeting = await calendarGrid().findByRole("button", { name: /^Standup/ });
    expect(meeting.style.getPropertyValue("--calendar-color")).toBe("");

    const list = screen.getByRole("region", { name: "Calendars" });
    const trigger = within(list).getByRole("button", { name: "Options for joel@example.com" });
    expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(within(list).getByRole("button", { name: "Options for Holidays in United States" })).toBeInTheDocument();

    fireEvent.click(trigger);
    const palette = within(list).getByRole("menu", { name: "Color for joel@example.com" });
    // Sixteen colors plus Default.
    expect(within(palette).getAllByRole("menuitemradio")).toHaveLength(17);
    expect(within(palette).getByRole("menuitemradio", { name: "Default" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(within(palette).getByRole("menuitemradio", { name: "Teal" }));

    expect(within(list).queryByRole("menu")).not.toBeInTheDocument();
    expect(calendarGrid().getByRole("button", { name: /^Standup/ }).style.getPropertyValue("--calendar-color")).toBe("#1f9a8f");
    expect(screen.getByRole("button", { name: "Offsite" }).style.getPropertyValue("--calendar-color")).toBe("#1f9a8f");
    expect(calendarGrid().getByRole("button", { name: /^Parade/ }).style.getPropertyValue("--calendar-color")).toBe("");
    expect(JSON.parse(localStorage.getItem(CALENDAR_COLORS_KEY)!)).toEqual({ "joel@example.com": { primary: "teal" } });

    fireEvent.click(trigger);
    expect(within(list).getByRole("menuitemradio", { name: "Teal" })).toHaveAttribute("aria-checked", "true");
    expect(within(list).getByRole("menuitemradio", { name: "Red" })).toHaveAttribute("aria-checked", "false");
    expect(within(list).getByRole("menuitemradio", { name: "Default" })).toHaveAttribute("aria-checked", "false");

    fireEvent.click(within(list).getByRole("menuitemradio", { name: "Default" }));
    expect(calendarGrid().getByRole("button", { name: /^Standup/ }).style.getPropertyValue("--calendar-color")).toBe("");
    expect(screen.getByRole("button", { name: "Offsite" }).style.getPropertyValue("--calendar-color")).toBe("");
    expect(localStorage.getItem(CALENDAR_COLORS_KEY)).toBeNull();
  });

  it("closes the calendar color menu on Escape or a click elsewhere", async () => {
    renderWeek();
    await screen.findByText("Sun 20");
    const list = screen.getByRole("region", { name: "Calendars" });
    const trigger = within(list).getByRole("button", { name: "Options for joel@example.com" });

    fireEvent.click(trigger);
    expect(within(list).getByRole("menu")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(within(list).queryByRole("menu")).not.toBeInTheDocument();

    fireEvent.click(trigger);
    fireEvent.mouseDown(screen.getByText("Sun 20"));
    expect(within(list).queryByRole("menu")).not.toBeInTheDocument();
    expect(localStorage.getItem(CALENDAR_COLORS_KEY)).toBeNull();
  });

  it("offers recovery when the schedule cannot be loaded", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const listScheduleEvents = vi.mocked(mailClient.listScheduleEvents).mockRejectedValue(new Error("offline"));
    const { props } = renderWeek();

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Calendar couldn’t be loaded");

    listScheduleEvents.mockResolvedValue({ events: [], errors: [] });
    fireEvent.click(within(alert).getByRole("button", { name: "Try Again" }));
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());

    listScheduleEvents.mockRejectedValue(new Error("offline"));
    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    fireEvent.click(within(await screen.findByRole("alert")).getByRole("button", { name: "Calendar Accounts" }));
    expect(props.onOpenSettings).toHaveBeenCalled();
  });

  it("restores and persists the grid scroll position", async () => {
    localStorage.setItem("threestrands.calendarWeek.scrollTop", "320");
    const { container } = renderWeek();
    await screen.findByText("Sun 20");

    const scroller = container.querySelector(".calendar-week-scroll") as HTMLElement;
    expect(scroller.scrollTop).toBe(320);

    fireEvent.scroll(scroller, { target: { scrollTop: 448 } });
    expect(localStorage.getItem("threestrands.calendarWeek.scrollTop")).toBe("448");
  });
  it("defaults to Workweek, persists range choices and keeps selected event details in the rail", async () => {
    localStorage.removeItem("threestrands.calendar.range");
    const {container}=renderWeek();
    expect(rangeButton("Workweek")).toHaveAttribute("aria-pressed", "true");
    // The range is a row of toggle buttons; the read-only workspace holds no select.
    expect(container.querySelector("select")).toBeNull();
    expect(container.querySelectorAll(".calendar-week-day-label")).toHaveLength(5);
    expect(screen.getByText("Mon 21")).toBeInTheDocument();
    fireEvent.click(rangeButton("3-Day"));
    expect(rangeButton("3-Day")).toHaveAttribute("aria-pressed", "true");
    expect(rangeButton("Workweek")).toHaveAttribute("aria-pressed", "false");
    expect(container.querySelectorAll(".calendar-week-day-label")).toHaveLength(3);
    expect(screen.getByText("Tue 22")).toBeInTheDocument();
    expect(localStorage.getItem("threestrands.calendar.range")).toBe("threeDays");
    fireEvent.click(rangeButton("Day"));
    expect(container.querySelectorAll(".calendar-week-day-label")).toHaveLength(1);
    await waitFor(()=>expect(mailClient.listScheduleEvents).toHaveBeenCalledWith(new Date(2026,8,22).toISOString(),new Date(2026,8,23).toISOString(),expect.any(String)));
  });

  it("opens a weekend event in Day without overwriting the saved Workweek preference", async () => {
    localStorage.setItem("threestrands.calendar.range","workweek");
    const weekend=event("weekend","2026-09-26T10:00:00","2026-09-26T11:00:00","Weekend review");
    render(<CalendarWeekView anchor={new Date(2026,8,26)} initialEvent={weekend} onAnchorChange={vi.fn()} accounts={accounts} calendars={calendars} onToggleCalendar={vi.fn()} onAddCalendarAccount={vi.fn()} onOpenSettings={vi.fn()} />);
    expect(rangeButton("Day")).toHaveAttribute("aria-pressed", "true");
    expect(localStorage.getItem("threestrands.calendar.range")).toBe("workweek");
    expect(await screen.findByRole("region",{name:"Weekend review details"})).toBeInTheDocument();
  });

});
