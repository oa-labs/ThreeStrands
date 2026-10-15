import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeetingScheduler } from "./MeetingScheduler";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences } from "./domain";
import { SLOTS_PER_DAY, type SchedulePlan } from "./scheduling";

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
    onAddToCalendar: vi.fn(), onReplyWithTimes: vi.fn(), onConfirmTime: vi.fn(), onOpenCalendarSettings: vi.fn(),
  };
  render(<MeetingScheduler plan={plan} preferences={preferences} calendarConnected {...handlers} {...overrides} />);
  return handlers;
}

describe("MeetingScheduler", () => {
  beforeEach(clearScheduleCache);
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.useRealTimers(); });

  // Pins only Date so range math is deterministic while findBy/waitFor keep real timeouts.
  // Tests run with TZ=America/New_York (vite.config.ts), so 14:00Z is 10:00 local.
  const NOW = "2026-09-29T14:00:00.000Z";
  const pinClock = () => { vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(NOW)); };
  const at = (iso: string, hours: number) => new Date(Date.parse(iso) + hours * 3_600_000).toISOString();

  it("confirms a free exact time and offers to add it, reply that it works, or find other times in place", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [candidate(50)], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    const handlers = renderScheduler(specific);

    expect(await screen.findByText("You’re free")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith({ start: specific.query!.start, end: specific.query!.end, timeZone: "America/New_York" });
    fireEvent.click(screen.getByRole("button", { name: "Add to Calendar" }));
    expect(handlers.onAddToCalendar).toHaveBeenCalledWith(specific.query);
    fireEvent.click(screen.getByRole("button", { name: "Reply “That Works”" }));
    expect(handlers.onConfirmTime).toHaveBeenCalledWith(specific.query);
    expect(screen.queryByRole("button", { name: "More Times" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find Other Times" }));
    expect(await screen.findByRole("group", { name: "Open times" })).toBeInTheDocument();
    expect(find).toHaveBeenCalledTimes(1);
  });

  it.each([
    { when: "on a later day", start: at(NOW, 26), rangeStart: "2026-09-30T04:00:00.000Z", rangeEnd: "2026-10-07T04:00:00.000Z" },
    { when: "later today", start: at(NOW, 2), rangeStart: NOW, rangeEnd: "2026-10-06T14:00:00.000Z" },
  ])("names the conflicting events and switches to a search for other times $when", async ({ start, rangeStart, rangeEnd }) => {
    pinClock();
    const conflicting: SchedulePlan = { query: { kind: "specific", start, end: at(start, 0.5) }, durationMinutes: 30, timeZoneAssumed: false };
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start, end: at(start, 0.5) }], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        { id: "crit", accountId: "you@example.com", title: "Design crit", allDay: false, start, end: at(start, 0.5) },
        { id: "holiday", accountId: "you@example.com", title: "Offsite", allDay: true, start: "2026-01-01", end: "2026-01-02" },
      ],
      errors: [],
    });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [{ start: at(NOW, 30), end: at(NOW, 30.75), status: "verified" }], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    renderScheduler(conflicting);

    expect(await screen.findByText("Conflicts with Design crit")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reply “That Works”" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find Other Times" }));
    expect(await screen.findByRole("group", { name: "Open times" })).toBeInTheDocument();
    // The search starts at the conflict's local midnight, but never in the past.
    expect(find).toHaveBeenCalledWith(expect.objectContaining({ rangeStart, rangeEnd, maxPerDay: SLOTS_PER_DAY }));
  });

  it("offers the first open time on each of the next days, selectable with ordinary buttons, without booking any", async () => {
    const days = [candidate(20), candidate(44), candidate(68), candidate(92)];
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: days, checkedCalendarCount: 1, totalCalendarCount: 1, errors: [],
    });
    const handlers = renderScheduler(range);

    const slots = within(await screen.findByRole("group", { name: "Open times" })).getAllByRole("button");
    expect(slots).toHaveLength(3);
    expect(find).toHaveBeenCalledWith({
      rangeStart: range.query!.start, rangeEnd: range.query!.end, maxPerDay: SLOTS_PER_DAY,
      preferences: { ...preferences, defaultDurationMinutes: 45 },
    });
    expect(screen.getByText("Using your time zone (America/New_York)")).toBeInTheDocument();
    expect(slots.every((slot) => slot.getAttribute("aria-pressed") === "true")).toBe(true);
    // Offered times are proposals; nothing is booked until the other side picks one.
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();

    fireEvent.click(slots[1]);
    fireEvent.click(slots[2]);
    expect(slots[0]).toHaveAttribute("aria-pressed", "true");
    expect(slots[1]).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByRole("button", { name: "Draft Reply With This Time" }));
    expect(handlers.onReplyWithTimes).toHaveBeenCalledWith([days[0]]);
    expect(handlers.onAddToCalendar).not.toHaveBeenCalled();

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
    pinClock();
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    renderScheduler({ query: null, durationMinutes: 30, timeZoneAssumed: false });
    expect(screen.getByText("That time has already passed.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find New Times" }));
    expect(await screen.findByText("No open times in your working hours for this range.")).toBeInTheDocument();
    expect(find).toHaveBeenCalledTimes(1);
    expect(find).toHaveBeenCalledWith(expect.objectContaining({ rangeStart: NOW, rangeEnd: "2026-10-06T14:00:00.000Z" }));
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
  it("adds an announced event as given, without offering other times or a reply", async () => {
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] } as never);
    const handlers = renderScheduler({ ...specific, announced: true });

    expect(await screen.findByText(/Conflicts with/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add to Calendar" }));
    expect(handlers.onAddToCalendar).toHaveBeenCalledWith(specific.query);
    expect(screen.queryByRole("button", { name: "Find Other Times" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reply “That Works”" })).not.toBeInTheDocument();
  });

  it("adds a date-only event as an all-day event without checking the calendar", () => {
    const check = vi.spyOn(mailClient, "checkProposedTime");
    const search = vi.spyOn(mailClient, "findAvailability");
    const handlers = renderScheduler({ query: null, day: "2026-11-07", durationMinutes: 30, timeZoneAssumed: false, announced: true });

    expect(screen.getByText("Sat, Nov 7 · Time not set")).toBeInTheDocument();
    expect(screen.queryByText("Checking your calendar…")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add to Calendar" }));
    expect(handlers.onAddToCalendar).toHaveBeenCalledWith({ start: "2026-11-07", end: "2026-11-07", allDay: true });
    expect(check).not.toHaveBeenCalled();
    expect(search).not.toHaveBeenCalled();
  });

  it("asks for a date for an announced event that has none, and never offers new times once it has passed", () => {
    const search = vi.spyOn(mailClient, "findAvailability");
    renderScheduler({ query: null, undated: true, durationMinutes: 30, timeZoneAssumed: false, announced: true });
    expect(screen.getByText("No date yet. Use Edit to set one.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
    cleanup();
    renderScheduler({ query: null, durationMinutes: 30, timeZoneAssumed: false, announced: true });
    expect(screen.getByText("This event has already happened.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Find New Times" })).not.toBeInTheDocument();
    expect(search).not.toHaveBeenCalled();
  });

  it("keeps its result when re-rendered with equal preferences", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
    const handlers = { onAddToCalendar: vi.fn(), onReplyWithTimes: vi.fn(), onConfirmTime: vi.fn(), onOpenCalendarSettings: vi.fn() };
    const { rerender } = render(<MeetingScheduler plan={specific} preferences={preferences} calendarConnected {...handlers} />);
    expect(await screen.findByText("You’re free")).toBeInTheDocument();
    // A synced copy with the same values must not re-check (and flash) the card.
    rerender(<MeetingScheduler plan={specific} preferences={{ ...preferences, workingWindows: preferences.workingWindows.map((window) => ({ ...window })) }} calendarConnected {...handlers} />);
    expect(screen.getByText("You’re free")).toBeInTheDocument();
    expect(screen.queryByText("Checking your calendar…")).not.toBeInTheDocument();
    expect(check).toHaveBeenCalledTimes(1);
    // A real change still re-checks.
    rerender(<MeetingScheduler plan={specific} preferences={{ ...preferences, timeZone: "America/Chicago" }} calendarConnected {...handlers} />);
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
  });
  describe("adjusting a range search", () => {
    // Pinned Tuesday 2026-09-29 10:00 New York; slots are in local hours on later days.
    const slotAt = (day: number, hour: number): AvailabilityCandidate => {
      const start = new Date(2026, 8, day, hour);
      return { start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString(), status: "verified" };
    };
    const week: SchedulePlan = { query: { kind: "range", start: "2026-09-30T04:00:00.000Z", end: "2026-10-07T04:00:00.000Z" }, durationMinutes: 30, timeZoneAssumed: false };
    const label = (hour: number) => new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone: "America/New_York" }).format(new Date(2026, 8, 1, hour));

    it("spreads the first times across days, then shows each day's later times on request", async () => {
      pinClock();
      vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
        // Earliest first, as the calendar search returns them; day 31 is Oct 1.
        candidates: [slotAt(30, 9), slotAt(30, 10), slotAt(30, 11), slotAt(31, 9)],
        checkedCalendarCount: 1, totalCalendarCount: 1, errors: [],
      });
      renderScheduler(week);

      const group = await screen.findByRole("group", { name: "Open times" });
      // Sep 30's first slot, then Oct 1's first, before Sep 30's second.
      expect(within(group).getAllByRole("button").map((slot) => slot.textContent)).toEqual([
        expect.stringContaining(label(9)), expect.stringContaining(label(10)), expect.stringContaining(label(9)),
      ]);
      expect(within(group).getAllByRole("button")[0]).toHaveTextContent("Sep 30");
      expect(within(group).getAllByRole("button")[2]).toHaveTextContent("Oct 1");
      fireEvent.click(screen.getByRole("button", { name: "Show More" }));
      const all = within(group).getAllByRole("button");
      expect(all).toHaveLength(4);
      // Newly shown times start unselected; the first three stay selected.
      expect(all.map((slot) => slot.getAttribute("aria-pressed"))).toEqual(["true", "true", "false", "true"]);
      expect(screen.queryByRole("button", { name: "Show More" })).not.toBeInTheDocument();
    });

    it("narrows the search to mornings or afternoons of the working hours, and back", async () => {
      pinClock();
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [slotAt(30, 9)], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
      renderScheduler(week);
      await screen.findByRole("group", { name: "Open times" });
      const windowsOf = (call: number) => find.mock.calls[call][0].preferences.workingWindows;
      expect(windowsOf(0)[0]).toEqual({ weekday: 1, start: "09:00", end: "17:00" });

      fireEvent.click(screen.getByRole("button", { name: "Afternoons" }));
      await waitFor(() => expect(find).toHaveBeenCalledTimes(2));
      expect(windowsOf(1)[0]).toEqual({ weekday: 1, start: "12:00", end: "17:00" });
      expect(screen.getByRole("button", { name: "Afternoons" })).toHaveAttribute("aria-pressed", "true");

      fireEvent.click(screen.getByRole("button", { name: "Mornings" }));
      await waitFor(() => expect(find).toHaveBeenCalledTimes(3));
      expect(windowsOf(2)[0]).toEqual({ weekday: 1, start: "09:00", end: "12:00" });
      expect(screen.getByRole("button", { name: "Afternoons" })).toHaveAttribute("aria-pressed", "false");

      fireEvent.click(screen.getByRole("button", { name: "Mornings" }));
      await waitFor(() => expect(find).toHaveBeenCalledTimes(4));
      expect(windowsOf(3)[0]).toEqual({ weekday: 1, start: "09:00", end: "17:00" });
    });

    it("moves to the following range and back, naming the days searched", async () => {
      pinClock();
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, errors: [] });
      renderScheduler(week);
      expect(await screen.findByText("Wed, Sep 30 – Tue, Oct 6")).toBeInTheDocument();
      // An empty range can still be adjusted.
      expect(screen.queryByRole("button", { name: "Earlier" })).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole("button", { name: "Later" }));
      expect(await screen.findByText("Wed, Oct 7 – Tue, Oct 13")).toBeInTheDocument();
      expect(find).toHaveBeenLastCalledWith(expect.objectContaining({ rangeStart: "2026-10-07T04:00:00.000Z", rangeEnd: "2026-10-14T04:00:00.000Z" }));

      fireEvent.click(screen.getByRole("button", { name: "Earlier" }));
      expect(await screen.findByText("Wed, Sep 30 – Tue, Oct 6")).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Earlier" })).not.toBeInTheDocument();
    });
  });
});
