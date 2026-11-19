import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearScheduleCache } from "./calendarScheduleCache";
import { saveCalendarColors } from "./calendarColors";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, ScheduleEvent } from "./domain";
import { InvitationDay } from "./InvitationDay";

const day = new Date(2027, 0, 5);
const at = (hour: number, minute = 0) => new Date(2027, 0, 5, hour, minute).toISOString();
const invitation = (overrides: Partial<CalendarEventPreview> = {}): CalendarEventPreview => ({
  uid: "summit@example.com", title: "Mid-Year Summit", start: at(14), end: at(15), allDay: false, timeZone: null,
  location: null, description: null, organizer: "Chapter", attendeeCount: 10, recurring: false, status: "CONFIRMED", ...overrides,
});
const scheduled = (id: string, start: string, end: string, overrides: Partial<ScheduleEvent> = {}): ScheduleEvent => ({
  id, accountId: "you@example.com", title: `Event ${id}`, allDay: false, start, end, ...overrides,
});

function renderDay(events: ScheduleEvent[], event = invitation(), errors: string[] = []) {
  const listScheduleEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events, errors });
  const onShowDay = vi.fn();
  const onOpenEvent = vi.fn();
  const view = render(<InvitationDay invitation={event} day={day} timeZone="UTC" onShowDay={onShowDay} onOpenEvent={onOpenEvent} />);
  return { ...view, listScheduleEvents, onShowDay, onOpenEvent };
}

const titles = (section: HTMLElement) => within(section).getAllByRole("button", { name: /^Event / }).map((button) => button.querySelector("strong")?.textContent);

describe("InvitationDay", () => {
  beforeEach(clearScheduleCache);
  afterEach(() => { cleanup(); saveCalendarColors({}); vi.restoreAllMocks(); });

  it("shows the invitation among nearby event cards, marks conflicts, and expands to the full day with all-day events first", async () => {
    const { listScheduleEvents, onOpenEvent } = renderDay([
      scheduled("afternoon", at(14, 30), at(15, 30)),
      scheduled("morning", at(9), at(10)),
      scheduled("holiday", "2027-01-05", "2027-01-06", { allDay: true }),
      scheduled("next-day", new Date(2027, 0, 6, 9).toISOString(), new Date(2027, 0, 6, 10).toISOString()),
      // Back to back with the invitation is not an overlap.
      scheduled("before", at(13), at(14)),
    ]);

    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(listScheduleEvents).toHaveBeenCalledWith(day.toISOString(), new Date(2027, 0, 6).toISOString(), "UTC");
    expect(listScheduleEvents).toHaveBeenCalledTimes(1);
    expect(titles(section)).toEqual(["Event before", "Event afternoon"]);
    const previewRows = within(section).getAllByRole("listitem");
    expect(previewRows[0]).toHaveTextContent("Event before");
    expect(previewRows[1]).toHaveClass("slot-day-proposed");
    expect(previewRows[1]).toHaveTextContent("This time");
    expect(previewRows[1].querySelector(".slot-day-meta")?.textContent).toBe(new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone: "UTC" }).formatRange(new Date(at(14)), new Date(at(15))));
    expect(previewRows[2]).toHaveTextContent("Event afternoon");
    fireEvent.click(within(section).getByRole("button", { name: "Show Full Day" }));
    expect(titles(section)).toEqual(["Event holiday", "Event morning", "Event before", "Event afternoon"]);
    expect(within(section).getByText("Overlaps 1 event")).toBeInTheDocument();
    const overlapping = within(section).getByRole("button", { name: /Event afternoon/ });
    expect(overlapping.closest("li")).toHaveClass("slot-day-event");
    expect(within(overlapping).getByText("Conflicts with this time")).toHaveClass("slot-day-detail-conflict");
    for (const name of [/Event morning/, /Event before/, /Event holiday/]) {
      const card = within(section).getByRole("button", { name });
      expect(card.closest("li")).toHaveClass("slot-day-event");
      expect(within(card).queryByText("Conflicts with this time")).not.toBeInTheDocument();
    }
    fireEvent.click(overlapping);
    expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "afternoon" }));
    fireEvent.click(within(section).getByRole("button", { name: "Show Less" }));
    expect(titles(section)).toEqual(["Event before", "Event afternoon"]);
    expect(listScheduleEvents).toHaveBeenCalledTimes(1);
  });

  it("uses calendar colors and keeps locations and response status on the event cards", async () => {
    saveCalendarColors({ "you@example.com": { work: "sky", home: "green" } });
    const location = "Room 210 Wexford, a long location that stays available in full on hover";
    const events = [
      scheduled("before", at(12), at(13), { calendarId: "home", responseStatus: "accepted", location }),
      scheduled("during", at(14), at(15), { calendarId: "work", responseStatus: "needsAction" }),
      scheduled("after", at(16), at(17), { responseStatus: "tentative" }),
    ];
    const { onOpenEvent } = renderDay(events);
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    const before = within(section).getByRole("button", { name: /Event before/ });
    expect(before.closest("li")).toHaveStyle({ "--calendar-color": "#3a9d5d" });
    expect(before).toHaveTextContent("Going");
    expect(within(before).getByTitle(location)).toHaveClass("slot-day-meta");
    const during = within(section).getByRole("button", { name: /Event during/ });
    expect(during.closest("li")).toHaveStyle({ "--calendar-color": "#4a9be8" });
    expect(during).toHaveTextContent("Awaiting response");
    expect(within(section).getByRole("button", { name: /Event after/ })).toHaveTextContent("Maybe");
    fireEvent.click(before);
    expect(onOpenEvent).toHaveBeenCalledWith(events[0]);
  });

  it("leaves out the invitation's own copy on the calendar and events the user declined", async () => {
    renderDay([
      scheduled("own-copy", at(14), at(15), { uid: " SUMMIT@example.com ", responseStatus: "needsAction" }),
      scheduled("declined", at(14), at(15), { responseStatus: "declined" }),
      scheduled("kept", at(9), at(10), { responseStatus: "accepted" }),
    ]);

    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(titles(section)).toEqual(["Event kept"]);
    expect(within(section).getByRole("button", { name: /Event kept/ })).toHaveTextContent("Going");
    // Neither left-out event counts against the invitation's time.
    const shownTime = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit", timeZone: "UTC" }).formatRange(new Date(at(14)), new Date(at(15)));
    expect(within(section).getByText(/^Free /).textContent).toBe(`Free ${shownTime}`);
    expect(within(section).getByText("This time").nextElementSibling?.textContent).toBe(shownTime);
  });

  it("names the day's working locations once on the date line instead of listing them as events", async () => {
    renderDay([
      scheduled("office-work", "2027-01-05", "2027-01-06", { allDay: true, title: "Office", eventType: "workingLocation" }),
      scheduled("office-personal", "2027-01-05", "2027-01-06", { allDay: true, title: "office ", eventType: "workingLocation", accountId: "me@example.com" }),
      scheduled("home", at(15), at(18), { title: "Home", eventType: "workingLocation" }),
      scheduled("morning", at(9), at(10)),
      scheduled("away", at(14), at(15), { eventType: "outOfOffice" }),
    ]);

    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(titles(section)).toEqual(["Event morning", "Event away"]);
    expect(section.querySelector(".context-invitation-day-place")).toHaveTextContent(/^Office, Home$/);
    // A working location does not take up the invitation's time; being out of office does.
    expect(within(section).getByText("Overlaps 1 event")).toBeInTheDocument();
    // The note is a flex row that spreads its children apart, so the date line is one child that reads as one phrase.
    const note = section.querySelector(".context-section-note");
    expect(note?.children).toHaveLength(1);
    expect(note).toHaveTextContent(/^\w{3}, \w{3} \d+ · Office, Home · Overlaps 1 event$/);
  });

  it("calls a day with only a working location free", async () => {
    renderDay([scheduled("office", "2027-01-05", "2027-01-06", { allDay: true, title: "Office", eventType: "workingLocation" })],
      invitation({ allDay: true, start: "2027-01-05", end: "2027-01-06" }));
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(within(section).queryAllByRole("button", { name: /Office/ })).toHaveLength(0);
    expect(within(section).getByText("Nothing else on your calendar")).toBeInTheDocument();
  });

  it("says when an all-day invitation's day has nothing else, without claiming a free time", async () => {
    renderDay([], invitation({ allDay: true, start: "2027-01-05", end: "2027-01-06" }));
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(within(section).getByText("Nothing else on your calendar")).toBeInTheDocument();
    expect(within(section).queryByText(/^Free /)).not.toBeInTheDocument();
    expect(within(section).queryByText("This time")).not.toBeInTheDocument();
  });

  it("uses the same event cards for an all-day invitation, without inventing a timed slot", async () => {
    const { onOpenEvent } = renderDay([
      scheduled("afternoon", at(14), at(15), { location: "Room 4B", responseStatus: "needsAction" }),
      scheduled("holiday", "2027-01-05", "2027-01-06", { allDay: true }),
    ], invitation({ allDay: true, start: "2027-01-05", end: "2027-01-06" }));
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(titles(section)).toEqual(["Event holiday", "Event afternoon"]);
    expect(within(section).getByRole("button", { name: /Event holiday/ })).toHaveTextContent("All day");
    const afternoon = within(section).getByRole("button", { name: /Event afternoon/ });
    expect(afternoon.closest("li")).toHaveClass("slot-day-event");
    expect(afternoon).toHaveTextContent("Room 4B · Awaiting response");
    expect(within(section).queryByText(/This time|Conflicts with this time|^Free /)).not.toBeInTheDocument();
    fireEvent.click(afternoon);
    expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "afternoon" }));
  });

  it("does not judge overlaps for times in a zone that could not be resolved", async () => {
    renderDay([scheduled("afternoon", at(14, 30), at(15, 30))],
      invitation({ start: "2027-01-05T14:00:00", end: "2027-01-05T15:00:00", timeZone: "Olympus Mons Time" }));
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(titles(section)).toEqual(["Event afternoon"]);
    expect(within(section).queryByText(/Overlaps|^Free /)).not.toBeInTheDocument();
    expect(within(section).queryByText("This time")).not.toBeInTheDocument();
    expect(within(section).getByRole("button", { name: /Event afternoon/ }).closest("li")).toHaveClass("slot-day-event");
  });

  it("shows the whole day in the schedule", async () => {
    const { onShowDay } = renderDay([]);
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    fireEvent.click(within(section).getByRole("button", { name: "Show Day in Schedule" }));
    expect(onShowDay).toHaveBeenCalledWith(day);
  });

  it("lists what loaded, without calling the time free, when some calendars fail", async () => {
    renderDay([scheduled("morning", at(9), at(10))], invitation(), ["work calendar unavailable"]);
    const section = await screen.findByRole("region", { name: "Your calendar that day" });
    expect(titles(section)).toEqual(["Event morning"]);
    expect(within(section).queryByText(/^Free /)).not.toBeInTheDocument();
  });

  it("renders nothing when the day's schedule cannot be loaded", async () => {
    const { listScheduleEvents } = renderDay([], invitation(), ["calendar unavailable"]);
    await vi.waitFor(() => expect(listScheduleEvents).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByRole("region", { name: "Your calendar that day" })).not.toBeInTheDocument();
  });
});
