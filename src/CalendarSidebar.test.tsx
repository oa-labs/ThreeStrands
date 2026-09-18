import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { scheduleRequestFor } from "./CalendarSidebar";
import { mailClient } from "./data/client";

describe("calendar sidebar", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
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
    const listEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue([
      {
        id: "planning",
        accountId: "calendar@example.com",
        title: "Product planning",
        start: "2026-09-18T10:00:00-07:00",
        end: "2026-09-18T10:30:00-07:00",
        allDay: false,
      },
    ]);
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

  it("surfaces Calendar API failures and links back to account settings", async () => {
    vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([
      {
        email: "calendar@example.com",
        connectedAt: "2026-09-18T00:00:00Z",
        status: "connected",
      },
    ]);
    vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValue(
      new Error("Google Calendar returned 403 Forbidden: API has not been used"),
    );
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    fireEvent.keyDown(window, { key: "T" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("403 Forbidden");
    fireEvent.click(screen.getByRole("button", { name: "Calendar Accounts" }));
    expect(await screen.findByRole("region", { name: "Calendar Accounts" })).toBeInTheDocument();
  });

  it("includes the T shortcut in keyboard help", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Keyboard shortcuts (?)" }));

    const dialog = await screen.findByRole("dialog", { name: "Keyboard shortcuts" });
    expect(dialog).toHaveTextContent("Open today’s schedule");
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
