import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { CALENDAR_SCROLL_TOP_KEY, CalendarSidebar, hasWorkingHoursOnDate, scheduleRequestFor } from "./CalendarSidebar";
import { mailClient } from "./data/client";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

describe("calendar sidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.mocked(openUrl).mockClear();
    localStorage.removeItem(CALENDAR_SCROLL_TOP_KEY);
    localStorage.removeItem("threestrands.settings.availabilityPreferences");
  });

  it("recognizes whether a date has configured working hours", () => {
    const preferences = { workingWindows: [{ weekday: 1, start: "09:00", end: "17:00" }] };
    expect(hasWorkingHoursOnDate(new Date(2026, 8, 21), preferences)).toBe(true);
    expect(hasWorkingHoursOnDate(new Date(2026, 8, 20), preferences)).toBe(false);
  });

  it("routes T to Calendar Accounts until a calendar is connected", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([]);
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "t" });

    expect(await screen.findByRole("region", { name: "Calendar Accounts" })).toBeInTheDocument();
    expect(screen.getByText("No calendars connected")).toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "Calendar schedule" })).not.toBeInTheDocument();
  });

  it("loads live events in the right sidebar and closes it with Escape", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    const listEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        {
          id: "planning",
          accountId: "calendar@example.com",
          title: "Product planning",
          start: "2026-09-18T10:00:00-07:00",
          end: "2026-09-18T10:30:00-07:00",
          allDay: false,
        },
      ],
      errors: [],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });

    const sidebar = await screen.findByRole("complementary", { name: "Calendar schedule" });
    await waitFor(() => expect(sidebar).toHaveTextContent("Product planning"));
    expect(listEvents).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("complementary", { name: "Calendar schedule" })).not.toBeInTheDocument(),
    );
  });

  it("toggles the schedule sidebar closed when T is pressed again", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });
    await screen.findByRole("complementary", { name: "Calendar schedule" });

    fireEvent.keyDown(window, { key: "T" });
    await waitFor(() =>
      expect(screen.queryByRole("complementary", { name: "Calendar schedule" })).not.toBeInTheDocument(),
    );
  });

  it("moves the schedule day with - and = shortcuts", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    const listEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });
    await screen.findByRole("complementary", { name: "Calendar schedule" });
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(1));
    const initialTimeMin = listEvents.mock.calls[0][0];

    fireEvent.keyDown(window, { key: "-" });
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(2));
    const previousTimeMin = listEvents.mock.calls[1][0];
    expect(new Date(previousTimeMin).getTime()).toBeLessThan(new Date(initialTimeMin).getTime());

    fireEvent.keyDown(window, { key: "=" });
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(3));
    expect(listEvents.mock.calls[2][0]).toBe(initialTimeMin);
  });

  function renderSwipeSidebar() {
    const listEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    render(
      <CalendarSidebar
        onClose={vi.fn()}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: [],
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
      />,
    );
    const sidebar = screen.getByRole("complementary", { name: "Calendar schedule" });
    const dayOf = (call: number) => new Date(listEvents.mock.calls[call][0]).getTime();
    return { listEvents, sidebar, dayOf };
  }

  it("moves one day per horizontal trackpad swipe", async () => {
    const { listEvents, sidebar, dayOf } = renderSwipeSidebar();
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(1));

    // One gesture, including its momentum tail, advances exactly one day.
    for (let step = 0; step < 6; step += 1) fireEvent.wheel(sidebar, { deltaX: 30, deltaY: 2 });
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(2));
    expect(dayOf(1)).toBeGreaterThan(dayOf(0));

    await new Promise((resolve) => setTimeout(resolve, 300));
    fireEvent.wheel(sidebar, { deltaX: -40 });
    fireEvent.wheel(sidebar, { deltaX: -40 });
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(3));
    expect(dayOf(2)).toBe(dayOf(0));
  });

  it("ignores vertical scrolling, short horizontal nudges, and pinch zoom", async () => {
    const { listEvents, sidebar } = renderSwipeSidebar();
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(1));

    fireEvent.wheel(sidebar, { deltaX: 80, deltaY: 120 });
    fireEvent.wheel(sidebar, { deltaX: 200, ctrlKey: true });
    fireEvent.wheel(sidebar, { deltaX: 20 });
    fireEvent.wheel(sidebar, { deltaX: 20 });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(listEvents).toHaveBeenCalledTimes(1);
  });

  it("moves the day with horizontal touch swipes but not taps or vertical drags", async () => {
    const { listEvents, sidebar, dayOf } = renderSwipeSidebar();
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(1));
    const swipe = (fromX: number, toX: number, fromY = 200, toY = 200, pointerType = "touch") => {
      fireEvent.pointerDown(sidebar, { pointerId: 1, pointerType, isPrimary: true, clientX: fromX, clientY: fromY });
      fireEvent.pointerUp(sidebar, { pointerId: 1, pointerType, isPrimary: true, clientX: toX, clientY: toY });
    };

    swipe(200, 195);
    swipe(200, 150, 100, 300);
    swipe(300, 100, 200, 200, "mouse");
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(listEvents).toHaveBeenCalledTimes(1);

    swipe(300, 150);
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(2));
    expect(dayOf(1)).toBeGreaterThan(dayOf(0));

    swipe(150, 300);
    await waitFor(() => expect(listEvents).toHaveBeenCalledTimes(3));
    expect(dayOf(2)).toBe(dayOf(0));
  });

  it("renders short meetings compactly with title and time on one line", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        {
          id: "standup",
          accountId: "calendar@example.com",
          title: "Standup",
          start: "2026-09-18T09:00:00-07:00",
          end: "2026-09-18T09:15:00-07:00",
          allDay: false,
        },
        {
          id: "sync",
          accountId: "calendar@example.com",
          title: "Sync",
          start: "2026-09-18T10:00:00-07:00",
          end: "2026-09-18T10:30:00-07:00",
          allDay: false,
        },
        {
          id: "review",
          accountId: "calendar@example.com",
          title: "Review",
          start: "2026-09-18T11:00:00-07:00",
          end: "2026-09-18T12:00:00-07:00",
          allDay: false,
        },
      ],
      errors: [],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });
    await screen.findByRole("complementary", { name: "Calendar schedule" });
    await waitFor(() => expect(screen.getByText("Standup")).toBeInTheDocument());

    const standup = screen.getByText("Standup").closest("button");
    const sync = screen.getByText("Sync").closest("button");
    const review = screen.getByText("Review").closest("button");

    expect(standup).toHaveClass("calendar-schedule-event-compact", "calendar-schedule-event-tight");
    expect(sync).toHaveClass("calendar-schedule-event-compact");
    expect(sync).not.toHaveClass("calendar-schedule-event-tight");
    expect(review).not.toHaveClass("calendar-schedule-event-compact");
    const standupTime = standup?.querySelector("span")?.textContent ?? "";
    expect(standupTime.match(/\b(?:am|pm)\b/gi)).toHaveLength(1);
    expect(standupTime).toMatch(/\b(?:am|pm)$/i);
  });

  it("shows event details and dismisses the viewer before the sidebar", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        {
          id: "planning",
          accountId: "calendar@example.com",
          title: "Product planning",
          start: "2026-09-18T10:00:00-07:00",
          end: "2026-09-18T11:00:00-07:00",
          allDay: false,
          location: "Room 4B",
          description: "Review the fall roadmap.",
          conferenceUrl: "https://meet.google.com/abc-defg-hij",
        },
      ],
      errors: [],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.keyDown(window, { key: "T" });

    fireEvent.click(await screen.findByRole("button", { name: /Product planning/ }));
    const viewer = await screen.findByRole("dialog", { name: "Product planning details" });
    expect(viewer).toHaveTextContent("Room 4B");
    expect(viewer).toHaveTextContent("Review the fall roadmap.");
    expect(viewer).toHaveTextContent("calendar@example.com");

    fireEvent.click(screen.getByRole("link", { name: "Join video meeting" }));
    expect(openUrl).toHaveBeenCalledWith("https://meet.google.com/abc-defg-hij");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Product planning details" })).not.toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "Calendar schedule" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Product planning/ }));
    expect(screen.getByRole("dialog", { name: "Product planning details" })).toBeInTheDocument();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog", { name: "Product planning details" })).not.toBeInTheDocument();
  });

  it("restores the saved calendar scroll position after a remount", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });

    const firstRender = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.keyDown(window, { key: "T" });
    const firstSidebar = await screen.findByRole("complementary", { name: "Calendar schedule" });
    const firstGrid = firstSidebar.querySelector<HTMLElement>(".calendar-grid-scroll");
    expect(firstGrid).not.toBeNull();
    await waitFor(() => expect(firstGrid?.scrollTop).toBe(7 * 64));

    if (firstGrid) {
      firstGrid.scrollTop = 9 * 64;
      fireEvent.scroll(firstGrid);
    }
    expect(localStorage.getItem(CALENDAR_SCROLL_TOP_KEY)).toBe(String(9 * 64));
    firstRender.unmount();

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.keyDown(window, { key: "T" });
    const restoredSidebar = await screen.findByRole("complementary", { name: "Calendar schedule" });
    const restoredGrid = restoredSidebar.querySelector<HTMLElement>(".calendar-grid-scroll");
    await waitFor(() => expect(restoredGrid?.scrollTop).toBe(9 * 64));
  });

  it("shows a short Calendar API error without rendering the provider response", async () => {
    const diagnostics = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [],
      errors: [
        "calendar@example.com: Google Calendar returned 404 Not Found: <html><body>Provider error</body></html>",
      ],
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Calendar couldn’t be loaded. Try again or reconnect in Calendar Accounts.",
    );
    expect(alert).not.toHaveTextContent("404 Not Found");
    expect(alert).not.toHaveTextContent("Provider error");
    expect(diagnostics).toHaveBeenCalledWith(
      "Calendar schedule load failed:",
      expect.arrayContaining([expect.stringContaining("<html>")]),
    );
    expect(alert.closest(".calendar-grid")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Calendar Accounts" }));
    expect(await screen.findByRole("region", { name: "Calendar Accounts" })).toBeInTheDocument();
  });

  it("persists an explicit calendar selection, including empty", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([
      {
        id: "primary",
        accountId: "calendar@example.com",
        name: "Personal",
        primary: true,
        selected: true,
      },
      {
        id: "team",
        accountId: "calendar@example.com",
        name: "Team",
        primary: false,
        selected: false,
      },
    ]);
    const setSelection = vi.spyOn(mailClient, "setCalendarSelection").mockResolvedValue([
      {
        id: "primary",
        accountId: "calendar@example.com",
        name: "Personal",
        primary: true,
        selected: false,
      },
      {
        id: "team",
        accountId: "calendar@example.com",
        name: "Team",
        primary: false,
        selected: false,
      },
    ]);
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    fireEvent.click(await screen.findByRole("button", { name: "Calendar Accounts" }));

    const primary = await screen.findByRole("checkbox", { name: "Personal (Primary)" });
    expect(primary).toBeChecked();
    fireEvent.click(primary);

    await waitFor(() =>
      expect(setSelection).toHaveBeenCalledWith("calendar@example.com", []),
    );
  });

  it("includes the T shortcut in keyboard help", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    expect(screen.queryByRole("button", { name: /Keyboard Shortcuts/ })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "?" });

    const dialog = await screen.findByRole("dialog", { name: "Keyboard Shortcuts" });
    expect(dialog).toHaveTextContent("Toggle Today’s Schedule");
    expect(dialog).toHaveTextContent("T");
  });

  it("checks availability and distinguishes verified candidate slots", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      { email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    const findAvailability = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: [
        { start: "2026-09-18T13:00:00Z", end: "2026-09-18T13:30:00Z", status: "verified" },
        { start: "2026-09-18T14:00:00Z", end: "2026-09-18T14:30:00Z", status: "verified" },
      ],
      checkedCalendarCount: 1,
      totalCalendarCount: 1,
      errors: [],
    });
    localStorage.setItem("threestrands.settings.availabilityPreferences", JSON.stringify({
      timeZone: "America/New_York",
      workingWindows: Array.from({ length: 7 }, (_, weekday) => ({ weekday, start: "09:00", end: "17:00" })),
      defaultDurationMinutes: 30,
      slotIncrementMinutes: 15,
    }));
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.keyDown(window, { key: "T" });
    fireEvent.click(await screen.findByRole("button", { name: "Check Schedule" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Check Availability" })).getByRole("button", { name: "Check Schedule" }));
    const candidates = await screen.findAllByRole("button", { name: /Verified/ });
    expect(candidates).toHaveLength(2);
    fireEvent.click(candidates[0]);
    fireEvent.click(candidates[1]);
    expect(candidates[0]).toHaveAttribute("aria-pressed", "true");
    expect(candidates[1]).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("2 times selected")).toBeInTheDocument();
    expect(findAvailability).toHaveBeenCalledTimes(1);
  });

  it("offers selected availability slots for a deterministic reply draft", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: [{ start: "2026-09-18T13:00:00Z", end: "2026-09-18T13:30:00Z", status: "verified" }],
      checkedCalendarCount: 1,
      totalCalendarCount: 1,
      errors: [],
    });
    const onDraftAvailability = vi.fn();
    render(
      <CalendarSidebar
        onClose={vi.fn()}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: [{ weekday: new Date().getDay(), start: "09:00", end: "17:00" }],
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
        onDraftAvailability={onDraftAvailability}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Check Schedule" }));
    fireEvent.click(within(await screen.findByRole("dialog", { name: "Check Availability" })).getByRole("button", { name: "Check Schedule" }));
    const candidate = await screen.findByRole("button", { name: /Verified/ });
    fireEvent.click(candidate);
    fireEvent.click(screen.getByRole("button", { name: "Draft Reply With Selected Times" }));
    expect(onDraftAvailability).toHaveBeenCalledWith([
      { start: "2026-09-18T13:00:00Z", end: "2026-09-18T13:30:00Z", status: "verified" },
    ]);
  });

  it("keeps availability parameters visible when the check fails", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    vi.spyOn(mailClient, "findAvailability").mockRejectedValue(new Error("Calendar check failed"));
    render(
      <CalendarSidebar
        onClose={vi.fn()}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: [{ weekday: new Date().getDay(), start: "09:00", end: "17:00" }],
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Check Schedule" }));
    const dialog = await screen.findByRole("dialog", { name: "Check Availability" });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Duration" }), { target: { value: "45" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Check Schedule" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Calendar check failed");
    expect(within(dialog).getByRole("combobox", { name: "Duration" })).toHaveValue("45");
  });

  it("hides availability controls when the selected day has no working hours", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    const selectedWeekday = new Date().getDay();
    render(
      <CalendarSidebar
        onClose={vi.fn()}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: [{ weekday: (selectedWeekday + 1) % 7, start: "09:00", end: "17:00" }],
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
      />,
    );

    await screen.findByRole("complementary", { name: "Calendar schedule" });
    expect(screen.queryByRole("region", { name: "Check Availability" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check Schedule" })).not.toBeInTheDocument();
  });

  it("hides availability controls on dates in the past", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    render(
      <CalendarSidebar
        onClose={vi.fn()}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: Array.from({ length: 7 }, (_, weekday) => ({ weekday, start: "09:00", end: "17:00" })),
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
      />,
    );

    expect(await screen.findByRole("button", { name: "Check Schedule" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Previous day (-)" }));
    expect(screen.queryByRole("region", { name: "Check Availability" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check Schedule" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Next day (=)" }));
    expect(await screen.findByRole("button", { name: "Check Schedule" })).toBeInTheDocument();
  });

  it("hides the close control and ignores Escape when embedded", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    const onClose = vi.fn();
    render(
      <CalendarSidebar
        embedded
        onClose={onClose}
        onOpenSettings={vi.fn()}
        availabilityPreferences={{
          timeZone: "America/New_York",
          workingWindows: [],
          defaultDurationMinutes: 30,
          slotIncrementMinutes: 15,
        }}
      />,
    );

    await screen.findByRole("complementary", { name: "Calendar schedule" });
    expect(screen.queryByRole("button", { name: "Close calendar" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("builds an exact local-day request", () => {
    const request = scheduleRequestFor(new Date(2026, 8, 18, 15, 30));
    const start = new Date(request.timeMin);
    const end = new Date(request.timeMax);

    expect(start.getHours()).toBe(0);
    expect(start.getMinutes()).toBe(0);
    expect(end.getTime() - start.getTime()).toBeGreaterThanOrEqual(23 * 60 * 60 * 1000);
    expect(end.getTime() - start.getTime()).toBeLessThanOrEqual(25 * 60 * 60 * 1000);
    expect(request.timeZone).toBeTruthy();
  });
});
