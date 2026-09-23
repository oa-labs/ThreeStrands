import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CalendarWeekView, monthGridDays } from "./CalendarWeekView";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ScheduleEvent } from "./domain";

const accounts: CalendarAccount[] = [
  { email: "joel@example.com", connectedAt: "2026-01-01T00:00:00Z", status: "connected" },
];
const calendars: CalendarOption[] = [
  { id: "primary", accountId: "joel@example.com", name: "joel@example.com", primary: true, selected: true },
  { id: "holidays", accountId: "joel@example.com", name: "Holidays in United States", primary: false, selected: true },
];

function renderWeek(overrides: Partial<Parameters<typeof CalendarWeekView>[0]> = {}) {
  const props = {
    accounts,
    calendars,
    onToggleCalendar: vi.fn(),
    onAddCalendarAccount: vi.fn(),
    onOpenSettings: vi.fn(),
    ...overrides,
  };
  return { ...render(<CalendarWeekView {...props} />), props };
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

  it("requests exactly the visible week and reloads when navigating", async () => {
    const listScheduleEvents = vi.mocked(mailClient.listScheduleEvents);
    renderWeek();
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(1));
    expect(new Date(listScheduleEvents.mock.calls[0][0])).toEqual(new Date(2026, 8, 20));
    expect(new Date(listScheduleEvents.mock.calls[0][1])).toEqual(new Date(2026, 8, 27));

    fireEvent.click(screen.getByRole("button", { name: "Next Week (=)" }));
    await waitFor(() => expect(listScheduleEvents).toHaveBeenCalledTimes(2));
    expect(new Date(listScheduleEvents.mock.calls[1][0])).toEqual(new Date(2026, 8, 27));
    await screen.findByText("Sun 27");

    fireEvent.click(screen.getByRole("button", { name: "Previous Week (-)" }));
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

    fireEvent.click(screen.getByRole("button", { name: "Next Week (=)" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Next Week (=)" }));
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
