import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { CalendarWeekView, monthGridDays } from "./CalendarWeekView";
import { startOfLocalDay } from "./calendarTime";
import { mailClient } from "./data/client";
import { clearScheduleCache } from "./calendarScheduleCache";
import { expectSharedButtons } from "./test/sharedButtons";
import type { CalendarAccount, CalendarOption, ScheduleEvent } from "./domain";

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
  return { ...render(<StatefulWeekView {...props} />), props };
}

function event(id: string, start: string, end: string, title = id): ScheduleEvent {
  return { id, accountId: "joel@example.com", title, start, end, allDay: false };
}

describe("CalendarWeekView", () => {
  beforeEach(() => {
    vi.setSystemTime(new Date(2026, 8, 22, 14, 30));
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
  });

  afterEach(() => {
    cleanup();
    clearScheduleCache();
    vi.useRealTimers();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("shows the seven Sunday-anchored days of the current week with the month title", async () => {
    renderWeek();
    await screen.findByRole("heading", { level: 1, name: "September 2026" });
    for (const label of ["Sun 20", "Mon 21", "Tue 22", "Wed 23", "Thu 24", "Fri 25", "Sat 26"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("names every account contributing selected calendars in the header", () => {
    const { container } = renderWeek({
      accounts: [...accounts, { ...accounts[0], email: "work@example.com" }],
      calendars: [...calendars, { id: "work", accountId: "work@example.com", name: "Work", primary: true, selected: true, writable: true }],
    });
    expect(container.querySelector(".calendar-week-header")).toHaveTextContent("Calendar · joel@example.com, work@example.com");
  });

  it("requests the visible week, preloads neighbors, and refreshes when navigating", async () => {
    const listScheduleEvents = vi.mocked(mailClient.listScheduleEvents);
    renderWeek();
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(3));
    expect(new Date(listScheduleEvents.mock.calls[0][0])).toEqual(new Date(2026, 8, 20));
    expect(new Date(listScheduleEvents.mock.calls[0][1])).toEqual(new Date(2026, 8, 27));

    fireEvent.click(screen.getByRole("button", { name: "Next week (=)" }));
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(5));
    expect(new Date(listScheduleEvents.mock.calls[3][0])).toEqual(new Date(2026, 8, 27));
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
    expect(screen.getByRole("button", { name: /Cached planning/ })).toBeInTheDocument();
    expect(screen.queryByText("Loading schedule…")).not.toBeInTheDocument();
    await act(async () => { resolve({ events: [], errors: [] }); });
    expect(screen.queryByRole("button", { name: /Cached planning/ })).not.toBeInTheDocument();
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

    const standup = await screen.findByRole("button", { name: /Standup/ });
    const review = screen.getByRole("button", { name: /Design review/ });
    expect(standup.style.width).toBe("50%");
    expect(standup.style.left).toBe("0%");
    expect(review.style.left).toBe("50%");
    expect(container.querySelectorAll(".calendar-week-column .calendar-schedule-event")).toHaveLength(2);

    fireEvent.click(standup);
    const viewer = await screen.findByRole("dialog", { name: "Standup details" });
    expect(within(viewer).getByText(/Tue, Sep 22/)).toBeInTheDocument();
  });

  it("opens a meeting's details when arriving from the conversation panel", async () => {
    const linked = event("linked", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Operations team meeting");
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [linked], errors: [] });
    const { container } = renderWeek({ initialEvent: linked });

    const viewer = await screen.findByRole("dialog", { name: "Operations team meeting details" });
    expect(within(viewer).getByText(/Tue, Sep 22/)).toBeInTheDocument();
    expect(container.querySelector(".calendar-week-scroll")?.scrollTop).toBeGreaterThan(0);
    const eventButton = await screen.findByRole("button", { name: "Operations team meeting" });
    expect(eventButton).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(eventButton);
    expect(screen.queryByRole("dialog", { name: "Operations team meeting details" })).not.toBeInTheDocument();
    fireEvent.click(eventButton);
    const reopened = await screen.findByRole("dialog", { name: "Operations team meeting details" });
    fireEvent.click(within(reopened).getByRole("button", { name: "Close Event Details" }));
    expect(screen.queryByRole("dialog", { name: "Operations team meeting details" })).not.toBeInTheDocument();
  });

  it("shows RSVP styling and changes one response through the event popup", async () => {
    const invited = { ...event("primary:invited", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Team sync"), calendarId: "primary", responseStatus: "needsAction" as const, canRespond: true };
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [invited], errors: [] });
    const update = vi.spyOn(mailClient, "updateCalendarResponse").mockResolvedValue({ ...invited, responseStatus: "accepted" });
    renderWeek();
    const button = await screen.findByRole("button", { name: "Team sync" });
    expect(button).toHaveAttribute("data-response-status", "needsAction");
    fireEvent.click(button);
    const dialog = screen.getByRole("dialog", { name: "Team sync details" });
    expect(dialog).toHaveTextContent("Awaiting response");
    const going = within(dialog).getByRole("group", { name: "Going?" });
    expect(within(going).getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(within(going).getByRole("button", { name: "Yes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(invited, "accepted"));
    await waitFor(() => expect(dialog).toHaveTextContent("Going"));
    expect(within(going).getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true");
  });

  it("keeps the prior RSVP and reports a failed change", async () => {
    const invited = { ...event("primary:invited", "2026-09-22T09:00:00", "2026-09-22T09:30:00", "Review"), calendarId: "primary", responseStatus: "tentative" as const, canRespond: true };
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [invited], errors: [] });
    vi.spyOn(mailClient, "updateCalendarResponse").mockRejectedValue(new Error("Calendar unavailable"));
    renderWeek();
    fireEvent.click(await screen.findByRole("button", { name: "Review" }));
    const dialog = screen.getByRole("dialog", { name: "Review details" });
    fireEvent.click(within(dialog).getByRole("button", { name: "No" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Calendar unavailable");
    expect(within(dialog).getByRole("button", { name: "Maybe" })).toHaveAttribute("aria-pressed", "true");
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
    const middle = screen.getByRole("button", { name: /Middle meeting/ });
    expect(parseFloat(early.style.left)).toBeCloseTo(100 / 3);
    expect(parseFloat(early.style.width)).toBeCloseTo(200 / 3);
    expect(parseFloat(middle.style.width)).toBeCloseTo(100 / 3);
    expect(early.querySelector("strong")).toHaveTextContent("Early review");
    expect(early.querySelector("span")).toBeNull();
    expect(early.title).toContain("9:00");
  });

  it("anchors event details to the time grid instead of spanning the whole screen", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({
      events: [event("a", "2026-09-22T09:00:00", "2026-09-22T10:00:00", "Standup")],
      errors: [],
    });
    const { container } = renderWeek();

    fireEvent.click(await screen.findByRole("button", { name: /Standup/ }));
    const viewer = await screen.findByRole("dialog", { name: "Standup details" });

    // Scoped to the grid column so it never overlaps the month/calendar rail,
    // and width-capped rather than pinned to both edges.
    expect(container.querySelector(".calendar-week-main")).toContainElement(viewer);
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

  it("opens an hour-long event from a clicked time and saves its details on the primary calendar", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("created", "2026-09-22T09:15:00", "2026-09-22T10:15:00", "Planning"));
    const { container } = renderWeek();
    const column = container.querySelectorAll<HTMLElement>(".calendar-week-column")[2]!;
    vi.spyOn(column, "getBoundingClientRect").mockReturnValue({ top: 0 } as DOMRect);

    fireEvent.pointerDown(column, { button: 0, pointerId: 1, clientY: 9.25 * 64 });
    fireEvent.pointerUp(column, { button: 0, pointerId: 1, clientY: 9.25 * 64 });
    const dialog = await screen.findByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-09-22T09:15");
    expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-09-22T10:15");
    expect(within(dialog).getByLabelText("Calendar")).toHaveValue("joel@example.com\nprimary");
    fireEvent.change(within(dialog).getByLabelText("Title"), { target: { value: "Planning" } });
    fireEvent.change(within(dialog).getByLabelText("Invite people"), { target: { value: "Ada@Example.com, bob@example.com" } });
    fireEvent.change(within(dialog).getByLabelText("Description"), { target: { value: "Review the plan" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));

    await waitFor(() => expect(create).toHaveBeenCalledWith({
      accountId: "joel@example.com", calendarId: "primary", title: "Planning",
      start: new Date(2026, 8, 22, 9, 15).toISOString(),
      end: new Date(2026, 8, 22, 10, 15).toISOString(),
      attendees: ["ada@example.com", "bob@example.com"], description: "Review the plan",
    }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "New event" })).not.toBeInTheDocument());
  });

  it("uses the dragged span and allows changing the calendar", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(event("created", "2026-09-22T09:00:00", "2026-09-22T11:30:00"));
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
});
