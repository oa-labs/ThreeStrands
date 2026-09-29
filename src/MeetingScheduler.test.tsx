import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeetingScheduler } from "./MeetingScheduler";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences } from "./domain";
import type { SchedulePlan } from "./scheduling";

const preferences: AvailabilityPreferences = {
  timeZone: "America/New_York",
  workingWindows: [1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })),
  defaultDurationMinutes: 30,
  slotIncrementMinutes: 30,
};
const later = (hours: number) => new Date(Date.now() + hours * 3_600_000).toISOString();
const specific: SchedulePlan = { query: { kind: "specific", start: later(26), end: later(26.5) }, durationMinutes: 30, timeZoneAssumed: false };
const range: SchedulePlan = { query: { kind: "range", start: later(1), end: later(7 * 24) }, durationMinutes: 45, timeZoneAssumed: true };
const candidate = (hours: number): AvailabilityCandidate => ({ start: later(hours), end: later(hours + 0.75), status: "verified" });

function renderScheduler(plan: SchedulePlan, overrides: Partial<Parameters<typeof MeetingScheduler>[0]> = {}) {
  const handlers = {
    onAddToCalendar: vi.fn(), onReplyWithTimes: vi.fn(), onConfirmTime: vi.fn(), onMoreTimes: vi.fn(), onOpenCalendarSettings: vi.fn(),
  };
  render(<MeetingScheduler plan={plan} preferences={preferences} calendarConnected {...handlers} {...overrides} />);
  return handlers;
}

describe("MeetingScheduler", () => {
  beforeEach(clearScheduleCache);
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("confirms a free exact time and offers to add it or reply that it works", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    const handlers = renderScheduler(specific);

    expect(await screen.findByText("You’re free")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith({ start: specific.query!.start, end: specific.query!.end, timeZone: "America/New_York" });
    fireEvent.click(screen.getByRole("button", { name: "Add to Calendar" }));
    expect(handlers.onAddToCalendar).toHaveBeenCalledWith(specific.query);
    fireEvent.click(screen.getByRole("button", { name: "Reply “That Works”" }));
    expect(handlers.onConfirmTime).toHaveBeenCalledWith(specific.query);
    fireEvent.click(screen.getByRole("button", { name: "More Times" }));
    expect(handlers.onMoreTimes).toHaveBeenCalledWith(new Date(specific.query!.start), 30);
  });

  it("names the conflicting events and switches to a search for other times", async () => {
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start: specific.query!.start, end: specific.query!.end }], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        { id: "crit", accountId: "you@example.com", title: "Design crit", allDay: false, start: specific.query!.start, end: specific.query!.end },
        { id: "holiday", accountId: "you@example.com", title: "Offsite", allDay: true, start: "2026-01-01", end: "2026-01-02" },
      ],
      errors: [],
    });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [candidate(30)], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    renderScheduler(specific);

    expect(await screen.findByText("Conflicts with Design crit")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reply “That Works”" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find Other Times" }));
    expect(await screen.findByRole("group", { name: "Open times" })).toBeInTheDocument();
    expect(find).toHaveBeenCalledWith(expect.objectContaining({ maxPerDay: 1 }));
  });

  it("offers the first open time on each of the next days, selectable with ordinary buttons", async () => {
    const days = [candidate(20), candidate(44), candidate(68), candidate(92)];
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: days, checkedCalendarCount: 1, totalCalendarCount: 1, errors: [],
    });
    const handlers = renderScheduler(range);

    const slots = within(await screen.findByRole("group", { name: "Open times" })).getAllByRole("button");
    expect(slots).toHaveLength(3);
    expect(find).toHaveBeenCalledWith({
      rangeStart: range.query!.start, rangeEnd: range.query!.end, maxPerDay: 1,
      preferences: { ...preferences, defaultDurationMinutes: 45 },
    });
    expect(screen.getByText("Using your time zone (America/New_York)")).toBeInTheDocument();
    expect(slots.every((slot) => slot.getAttribute("aria-pressed") === "true")).toBe(true);
    expect(screen.getByRole("button", { name: "Add to Calendar" })).toBeDisabled();

    fireEvent.click(slots[1]);
    fireEvent.click(slots[2]);
    expect(slots[0]).toHaveAttribute("aria-pressed", "true");
    expect(slots[1]).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByRole("button", { name: "Draft Reply With This Time" }));
    expect(handlers.onReplyWithTimes).toHaveBeenCalledWith([days[0]]);
    fireEvent.click(screen.getByRole("button", { name: "Add to Calendar" }));
    expect(handlers.onAddToCalendar).toHaveBeenCalledWith(days[0]);

    fireEvent.click(slots[0]);
    expect(screen.getByRole("button", { name: "Draft Reply With These Times" })).toBeDisabled();
  });

  it("does not check anything without a connected calendar", () => {
    const check = vi.spyOn(mailClient, "checkProposedTime");
    const handlers = renderScheduler(specific, { calendarConnected: false });
    expect(screen.getByText("Connect a calendar to check times for this meeting.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Calendar Settings" }));
    expect(handlers.onOpenCalendarSettings).toHaveBeenCalled();
    expect(check).not.toHaveBeenCalled();
  });

  it("explains a passed time and can search new times instead", async () => {
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    renderScheduler({ query: null, durationMinutes: 30, timeZoneAssumed: false });
    expect(screen.getByText("That time has already passed.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find New Times" }));
    expect(await screen.findByText("No open times in your working hours for this range.")).toBeInTheDocument();
    expect(find).toHaveBeenCalled();
  });

  it("keeps a failed check retryable", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime")
      .mockRejectedValueOnce(new Error("Google Calendar request failed (503)."))
      .mockResolvedValueOnce({ status: "partiallyChecked", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 2, errors: [] });
    renderScheduler(specific);
    expect(await screen.findByRole("alert")).toHaveTextContent("Google Calendar request failed (503).");
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(await screen.findByText("Free on the calendars that could be checked")).toBeInTheDocument();
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
  });
});
