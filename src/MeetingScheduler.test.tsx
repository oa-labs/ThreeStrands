import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeetingScheduler, uncheckedNote, zoneName } from "./MeetingScheduler";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { AvailabilityCandidate, AvailabilityPreferences } from "./domain";
import { SLOTS_PER_DAY, type SchedulePlan } from "./scheduling";

const preferences: AvailabilityPreferences = {
  timeZone: "America/New_York",
  workingWindows: [1, 2, 3, 4, 5].map((weekday) => ({ weekday, start: "09:00", end: "17:00" })),
  defaultDurationMinutes: 30,
  slotIncrementMinutes: 30,
  driveTimeBufferMinutes: 0,
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

  it("uses the drive time setting for exact checks and alternative time suggestions", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    renderScheduler(specific, { preferences: { ...preferences, driveTimeBufferMinutes: 30 } });
    await waitFor(() => expect(check).toHaveBeenCalledWith(expect.objectContaining({ driveTimeBufferMinutes: 30 })));
    fireEvent.click(await screen.findByRole("button", { name: "Find Other Times" }));
    await waitFor(() => expect(find).toHaveBeenCalledWith(expect.objectContaining({ preferences: expect.objectContaining({ driveTimeBufferMinutes: 30 }) })));
  });

  it("confirms a free exact time and offers to add it, reply that it works, or find other times in place", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [candidate(50)], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const handlers = renderScheduler(specific);

    expect(await screen.findByText("You’re free")).toBeInTheDocument();
    expect(check).toHaveBeenCalledWith({ start: specific.query!.start, end: specific.query!.end, timeZone: "America/New_York", driveTimeBufferMinutes: 0 });
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
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start, end: at(start, 0.5) }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [
        { id: "crit", accountId: "you@example.com", title: "Design crit", allDay: false, start, end: at(start, 0.5) },
        { id: "holiday", accountId: "you@example.com", title: "Offsite", allDay: true, start: "2026-01-01", end: "2026-01-02" },
      ],
      errors: [],
    });
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [{ start: at(NOW, 30), end: at(NOW, 30.75), status: "verified" }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
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
      candidates: days, checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [],
    });
    const handlers = renderScheduler(range);

    const slots = within(await screen.findByRole("group", { name: "Open times" })).getAllByRole("button");
    expect(slots).toHaveLength(3);
    expect(find).toHaveBeenCalledWith({
      rangeStart: range.query!.start, rangeEnd: range.query!.end, maxPerDay: SLOTS_PER_DAY,
      preferences: { ...preferences, defaultDurationMinutes: 45 },
    });
    expect(screen.getByText("Times in Eastern Time (the email didn’t say)")).toBeInTheDocument();
    // Every calendar answered, so nothing warns that a time may conflict.
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
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
    const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    renderScheduler({ query: null, durationMinutes: 30, timeZoneAssumed: false });
    expect(screen.getByText("That time has already passed.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Find New Times" }));
    expect(await screen.findByText("No open times in your working hours for this range.")).toBeInTheDocument();
    expect(find).toHaveBeenCalledTimes(1);
    expect(find).toHaveBeenCalledWith(expect.objectContaining({ rangeStart: NOW, rangeEnd: "2026-10-06T14:00:00.000Z" }));
  });

  it("names the calendars it could not check once for the whole list, not on each time", async () => {
    const partial = (hours: number): AvailabilityCandidate => ({ ...candidate(hours), status: "partiallyChecked" });
    vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: [partial(20), partial(44), partial(68)], checkedCalendarCount: 1, totalCalendarCount: 2, uncheckedCalendars: ["Team Shared"], errors: [],
    });
    const handlers = renderScheduler(range);
    const slots = within(await screen.findByRole("group", { name: "Open times" })).getAllByRole("button");
    expect(slots.some((slot) => /calendar/i.test(slot.textContent ?? ""))).toBe(false);
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent("Couldn’t check Team Shared, so these times may conflict with it.");
    fireEvent.click(within(note).getByRole("button", { name: "Calendar Settings" }));
    expect(handlers.onOpenCalendarSettings).toHaveBeenCalled();
  });

  it("says when no calendar could be checked for a range", async () => {
    vi.spyOn(mailClient, "findAvailability").mockResolvedValue({
      candidates: [{ ...candidate(20), status: "unverified" }], checkedCalendarCount: 0, totalCalendarCount: 0, uncheckedCalendars: [], errors: [],
    });
    renderScheduler(range);
    expect(await screen.findByRole("note")).toHaveTextContent("No calendar could be checked, so these times may conflict.");
  });

  it("names the calendars an exact time could not be checked against", async () => {
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({
      status: "partiallyChecked", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 3, uncheckedCalendars: ["Team Shared", "work@example.com"], errors: [],
    });
    renderScheduler(specific);
    expect(await screen.findByText("Free on the calendars that could be checked")).toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent("Couldn’t check Team Shared and work@example.com, so this time may conflict with them.");
  });

  it("describes unchecked calendars even when their names are unknown", () => {
    expect(uncheckedNote([], "these times")).toBe("Some calendars couldn’t be checked, so these times may conflict with them.");
  });

  it("names time zones the way people say them, keeping the zone ID when it has no name", () => {
    expect(zoneName("America/New_York")).toBe("Eastern Time");
    expect(zoneName("Europe/Berlin")).toBe("Central European Time");
    expect(zoneName("Not/AZone")).toBe("Not/AZone");
  });

  it("keeps a failed check retryable", async () => {
    const check = vi.spyOn(mailClient, "checkProposedTime")
      .mockRejectedValueOnce(new Error("Google Calendar request failed (503)."))
      .mockResolvedValueOnce({ status: "partiallyChecked", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 2, uncheckedCalendars: [], errors: [] });
    renderScheduler(specific);
    expect(await screen.findByRole("alert")).toHaveTextContent("Google Calendar request failed (503).");
    fireEvent.click(screen.getByRole("button", { name: "Try Again" }));
    expect(await screen.findByText("Free on the calendars that could be checked")).toBeInTheDocument();
    await waitFor(() => expect(check).toHaveBeenCalledTimes(2));
  });
  it("adds an announced event as given, without offering other times or a reply", async () => {
    vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
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
    const check = vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
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
  describe("recognizing an event already on the calendar", () => {
    const START = "2026-11-17T21:00:00.000Z";
    const announcedPlan: SchedulePlan = { query: { kind: "specific", start: START, end: at(START, 2) }, durationMinutes: 120, timeZoneAssumed: false, announced: true };
    const identity = { title: "Entrepreneur Day 2026", location: "Energy Innovation Center - 1435 Bedford Ave, Pittsburgh, PA 15219, USA" };
    const standing = { id: "standing", accountId: "you@example.com", title: "Premier Labs Bi-Weekly Member Meeting", allDay: false, start: at(START, -0.5), end: at(START, 0.5) };
    const added = { id: "added", accountId: "you@example.com", title: "Entrepreneur Day 2026", allDay: false, start: START, end: at(START, 2), location: "1435 Bedford Ave, Pittsburgh, PA 15219" };
    const conflicting = () => vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "conflicting", conflicts: [{ start: START, end: at(START, 2) }], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });

    it("says the event is already added, names only the other overlaps, and makes adding it again deliberate", async () => {
      conflicting();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [standing, added], errors: [] });
      const onOpenCalendarEvent = vi.fn();
      const handlers = renderScheduler(announcedPlan, { identity, onOpenCalendarEvent });

      expect(await screen.findByText("Already on your calendar")).toBeInTheDocument();
      expect(screen.getByText("Also overlaps Premier Labs Bi-Weekly Member Meeting")).toBeInTheDocument();
      expect(screen.queryByText(/Conflicts with/)).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Add Again" })).not.toBeInTheDocument();
      const open = screen.getByRole("button", { name: "Open in Calendar" });
      expect(open).toHaveClass("btn-primary");
      fireEvent.click(open);
      expect(onOpenCalendarEvent).toHaveBeenCalledWith(added);
      fireEvent.click(screen.getByRole("button", { name: "More actions" }));
      fireEvent.click(screen.getByRole("menuitem", { name: "Add Again" }));
      expect(handlers.onAddToCalendar).toHaveBeenCalledWith(announcedPlan.query);
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });

    it("lets a proposed meeting already on the calendar be confirmed when nothing else overlaps", async () => {
      conflicting();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [{ ...added, title: "Invitation: Entrepreneur Day" }], errors: [] });
      renderScheduler({ ...announcedPlan, announced: false }, { identity });

      expect(await screen.findByText("Already on your calendar")).toBeInTheDocument();
      expect(screen.queryByText(/Also overlaps/)).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Reply “That Works”" })).toBeInTheDocument();
      // Without a way to open the calendar, there is no button for it.
      expect(screen.queryByRole("button", { name: "Open in Calendar" })).not.toBeInTheDocument();
    });

    it("only suggests a match when the place and time agree but the title does not", async () => {
      conflicting();
      const venue = { ...added, id: "venue", title: "Pitch practice" };
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [standing, venue], errors: [] });
      renderScheduler(announcedPlan, { identity });

      expect(await screen.findByText("May already be on your calendar as Pitch practice")).toBeInTheDocument();
      expect(screen.getByText("Also overlaps Premier Labs Bi-Weekly Member Meeting")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Add to Calendar" })).toBeInTheDocument();
    });

    it("keeps an ordinary conflict when the overlapping event is a different one, or nothing names the suggestion", async () => {
      conflicting();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [standing, { ...added, start: at(START, 1) }], errors: [] });
      renderScheduler(announcedPlan, { identity });
      expect(await screen.findByText("Conflicts with Premier Labs Bi-Weekly Member Meeting, Entrepreneur Day 2026")).toBeInTheDocument();
      cleanup();
      clearScheduleCache();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [standing, added], errors: [] });
      renderScheduler(announcedPlan);
      expect(await screen.findByText("Conflicts with Premier Labs Bi-Weekly Member Meeting, Entrepreneur Day 2026")).toBeInTheDocument();
    });

    it("finds a date-only event already on its day, and adds as usual when it is not", async () => {
      const dayPlan: SchedulePlan = { query: null, day: "2026-11-07", durationMinutes: 30, timeZoneAssumed: false, announced: true };
      const check = vi.spyOn(mailClient, "checkProposedTime");
      const list = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
        events: [{ id: "race", accountId: "you@example.com", title: "Pittsburgh Marathon", allDay: true, start: "2026-11-07", end: "2026-11-08" }],
        errors: [],
      });
      const handlers = renderScheduler(dayPlan, { identity: { title: "Pittsburgh Marathon 2026" } });
      expect(await screen.findByText("Already on your calendar")).toBeInTheDocument();
      expect(screen.getByText("Sat, Nov 7 · All day")).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "More actions" }));
      fireEvent.click(screen.getByRole("menuitem", { name: "Add Again" }));
      expect(handlers.onAddToCalendar).toHaveBeenCalledWith({ start: "2026-11-07", end: "2026-11-07", allDay: true });
      expect(check).not.toHaveBeenCalled();
      cleanup();

      renderScheduler(dayPlan, { identity: { title: "Fall Gala" } });
      await waitFor(() => expect(list).toHaveBeenCalled());
      expect(screen.getByRole("button", { name: "Add to Calendar" })).toBeInTheDocument();
      expect(screen.queryByText("Already on your calendar")).not.toBeInTheDocument();
    });

    it("finds an event added from this email at another time that day, even when the suggested time is free", async () => {
      vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
      // 23:00Z is 6:00 PM in America/New_York, an hour after the suggested 4:00 PM ends.
      const moved = { ...added, start: "2026-11-17T23:00:00.000Z", end: "2026-11-18T01:00:00.000Z", sourceMessageId: "message-1" };
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [moved], errors: [] });
      renderScheduler(announcedPlan, { identity: { ...identity, sourceMessageId: "message-1" } });
      expect(await screen.findByText("Already on your calendar at 6:00 PM")).toBeInTheDocument();
      expect(screen.queryByText("You’re free")).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "More actions" }));
      expect(screen.getByRole("menuitem", { name: "Add Again" })).toBeInTheDocument();
    });

    it("gives a date-only event the time of the calendar copy it matched", async () => {
      // 15:00Z is 10:00 AM in America/New_York.
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
        events: [{ id: "council", accountId: "you@example.com", title: "Finance Council Meeting", allDay: false, start: "2026-11-07T15:00:00.000Z", end: "2026-11-07T16:00:00.000Z" }],
        errors: [],
      });
      renderScheduler({ query: null, day: "2026-11-07", durationMinutes: 30, timeZoneAssumed: false, announced: true }, { identity: { title: "Finance Council Meeting" } });
      expect(await screen.findByText("Sat, Nov 7 · 10:00 AM")).toBeInTheDocument();
      expect(screen.queryByText(/Time not set/)).not.toBeInTheDocument();
    });

    it("tells its caller about a match and puts the caller's actions in its row and menu", async () => {
      conflicting();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [added], errors: [] });
      const onMatchChange = vi.fn();
      const onEdit = vi.fn();
      renderScheduler(announcedPlan, {
        identity,
        onMatchChange,
        matchedActions: <button type="button" className="btn btn-sm">Dismiss</button>,
        moreActions: [{ label: "Edit", onSelect: onEdit }],
      });
      expect(await screen.findByRole("button", { name: "Dismiss" })).toBeInTheDocument();
      expect(onMatchChange).toHaveBeenLastCalledWith(added);
      fireEvent.click(screen.getByRole("button", { name: "More actions" }));
      expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Add Again", "Edit"]);
      fireEvent.click(screen.getByRole("menuitem", { name: "Edit" }));
      expect(onEdit).toHaveBeenCalledTimes(1);
    });

    it("keeps the caller's actions out of its row while the event is not on the calendar", async () => {
      vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
      const onMatchChange = vi.fn();
      renderScheduler(announcedPlan, { identity, onMatchChange, matchedActions: <button type="button" className="btn btn-sm">Dismiss</button> });
      expect(await screen.findByText("You’re free")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Add to Calendar" })).toHaveClass("btn-primary");
      expect(screen.queryByRole("button", { name: "Dismiss" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "More actions" })).not.toBeInTheDocument();
      expect(onMatchChange).not.toHaveBeenCalledWith(expect.objectContaining({ id: "added" }));
    });

    it("does not read the day for a free time with no email to look for", async () => {
      vi.spyOn(mailClient, "checkProposedTime").mockResolvedValue({ status: "free", conflicts: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
      const list = vi.spyOn(mailClient, "listScheduleEvents");
      renderScheduler(announcedPlan, { identity });
      expect(await screen.findByText("You’re free")).toBeInTheDocument();
      expect(list).not.toHaveBeenCalled();
    });

    it("checks once when re-rendered with an equal identity", async () => {
      const check = conflicting();
      vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [added], errors: [] });
      const handlers = { onAddToCalendar: vi.fn(), onReplyWithTimes: vi.fn(), onConfirmTime: vi.fn(), onOpenCalendarSettings: vi.fn() };
      const { rerender } = render(<MeetingScheduler plan={announcedPlan} preferences={preferences} calendarConnected identity={{ ...identity }} {...handlers} />);
      expect(await screen.findByText("Already on your calendar")).toBeInTheDocument();
      rerender(<MeetingScheduler plan={announcedPlan} preferences={preferences} calendarConnected identity={{ ...identity }} {...handlers} />);
      expect(screen.getByText("Already on your calendar")).toBeInTheDocument();
      expect(check).toHaveBeenCalledTimes(1);
    });
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
        checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [],
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
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [slotAt(30, 9)], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
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

    it("starts from the days and hours it was given, names them, and keeps the days when the hours change", async () => {
      pinClock();
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
      const onSearchChange = vi.fn();
      renderScheduler({ ...week, bounds: { weekdays: [4, 5], dayStart: "15:00", dayEnd: null } }, { onSearchChange });

      expect(await screen.findByText(`Wed, Sep 30 – Tue, Oct 6 · Thu, Fri · from ${label(15)}`)).toBeInTheDocument();
      // Thursday and Friday from 3 PM: other days' windows are left out.
      expect(find.mock.calls[0][0].preferences.workingWindows).toEqual([
        { weekday: 4, start: "15:00", end: "17:00" },
        { weekday: 5, start: "15:00", end: "17:00" },
      ]);
      expect(screen.getByRole("button", { name: "Mornings" })).toHaveAttribute("aria-pressed", "false");
      expect(screen.getByRole("button", { name: "Afternoons" })).toHaveAttribute("aria-pressed", "false");
      expect(onSearchChange).toHaveBeenLastCalledWith({ query: week.query, durationMinutes: 30, bounds: { weekdays: [4, 5], dayStart: "15:00", dayEnd: null } });

      fireEvent.click(screen.getByRole("button", { name: "Mornings" }));
      expect(await screen.findByText(`Wed, Sep 30 – Tue, Oct 6 · Thu, Fri · until ${label(12)}`)).toBeInTheDocument();
      expect(find.mock.calls[1][0].preferences.workingWindows).toEqual([
        { weekday: 4, start: "09:00", end: "12:00" },
        { weekday: 5, start: "09:00", end: "12:00" },
      ]);
      expect(onSearchChange).toHaveBeenLastCalledWith(expect.objectContaining({ bounds: { weekdays: [4, 5], dayStart: null, dayEnd: "12:00" } }));

      fireEvent.click(screen.getByRole("button", { name: "Mornings" }));
      expect(await screen.findByText("Wed, Sep 30 – Tue, Oct 6 · Thu, Fri")).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Later" }));
      await waitFor(() => expect(onSearchChange).toHaveBeenLastCalledWith(expect.objectContaining({
        query: { kind: "range", start: "2026-10-07T04:00:00.000Z", end: "2026-10-14T04:00:00.000Z" },
      })));
    });

    it("shows what it is given beside the adjustment chips", async () => {
      vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
      renderScheduler(range, { adjustInput: <input aria-label="Adjust times" /> });
      await screen.findByText("No open times in your working hours for this range.");
      expect(screen.getByRole("textbox", { name: "Adjust times" })).toBeInTheDocument();
    });

    it("moves to the following range and back, naming the days searched", async () => {
      pinClock();
      const find = vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
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
