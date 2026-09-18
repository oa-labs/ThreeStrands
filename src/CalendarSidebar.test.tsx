import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { CALENDAR_SCROLL_TOP_KEY, scheduleRequestFor } from "./CalendarSidebar";
import { mailClient } from "./data/client";

describe("calendar sidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    localStorage.removeItem(CALENDAR_SCROLL_TOP_KEY);
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

    const standup = screen.getByText("Standup").closest("article");
    const sync = screen.getByText("Sync").closest("article");
    const review = screen.getByText("Review").closest("article");

    expect(standup).toHaveClass("calendar-schedule-event-compact", "calendar-schedule-event-tight");
    expect(sync).toHaveClass("calendar-schedule-event-compact");
    expect(sync).not.toHaveClass("calendar-schedule-event-tight");
    expect(review).not.toHaveClass("calendar-schedule-event-compact");
    const standupTime = standup?.querySelector("span")?.textContent ?? "";
    expect(standupTime.match(/\b(?:am|pm)\b/gi)).toHaveLength(1);
    expect(standupTime).toMatch(/\b(?:am|pm)$/i);
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
    expect(firstGrid?.scrollTop).toBe(7 * 64);

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
    fireEvent.click(screen.getByRole("button", { name: "Keyboard shortcuts (?)" }));

    const dialog = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toHaveTextContent("Toggle today’s schedule");
    expect(dialog).toHaveTextContent("T");
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
