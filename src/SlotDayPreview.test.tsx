import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SlotDayPreview } from "./SlotDayPreview";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { TIGHT_BREAK_MINUTES } from "./scheduling";

// Tests run with TZ=America/New_York (vite.config.ts), matching the zone below.
const timeZone = "America/New_York";
const at = (hour: number, minute = 0) => new Date(2026, 9, 1, hour, minute).toISOString();
const event = (id: string, title: string, start: string, end: string, extra: Partial<ScheduleEvent> = {}): ScheduleEvent =>
  ({ id, accountId: "you@example.com", calendarId: "primary", title, allDay: false, start, end, ...extra });
const slot = { start: at(13), end: at(13, 30) };

function renderDay(events: ScheduleEvent[], target = slot) {
  const list = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events, errors: [] });
  render(<SlotDayPreview id="day" slot={target} timeZone={timeZone} />);
  return list;
}
const rows = async () => {
  const day = await screen.findByRole("region", { name: /October 1/ });
  await within(day).findByRole("list");
  return within(day).getAllByRole("listitem").map((item) => item.textContent);
};

describe("SlotDayPreview", () => {
  beforeEach(clearScheduleCache);
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("shows the meetings on either side of the time, where they are, and the free time between", async () => {
    renderDay([
      event("early", "Breakfast", at(8), at(8, 30)),
      event("before", "Design review", at(11), at(12, 30), { location: "Room 4B" }),
      event("after", "1:1 with Sam", at(14), at(14, 30), { location: "Zoom" }),
      event("late", "Gym", at(18), at(19)),
    ]);
    const shown = await rows();
    expect(shown).toEqual([
      expect.stringContaining("Design review"),
      "30 min after Design review",
      expect.stringContaining("This time"),
      "30 min before 1:1 with Sam",
      expect.stringContaining("1:1 with Sam"),
    ]);
    expect(shown[0]).toContain("Room 4B");
    expect(shown[4]).toContain("Zoom");
    // Only the neighbors show until the whole day is asked for.
    expect(screen.queryByText("Breakfast")).not.toBeInTheDocument();
  });

  it("marks a break shorter than the tight limit, but not one at the limit", async () => {
    renderDay([
      event("before", "Planning", at(12, 30), at(13)),
      event("after", "Interview", at(13, 30 + TIGHT_BREAK_MINUTES), at(14, 30)),
    ]);
    await rows();
    const tight = screen.getByText("Right after Planning");
    expect(tight).toHaveClass("slot-day-break-tight");
    expect(screen.getByText(`${TIGHT_BREAK_MINUTES} min before Interview`)).not.toHaveClass("slot-day-break-tight");
  });

  it("names events that overlap without blocking the time, and gives no break beside them", async () => {
    renderDay([
      event("before", "Planning", at(11), at(12)),
      event("hold", "Focus time", at(12, 45), at(13, 15)),
    ]);
    const shown = await rows();
    expect(shown).toEqual([expect.stringContaining("Planning"), expect.stringContaining("Focus time"), expect.stringContaining("This time")]);
    expect(shown[1]).toContain("Overlaps, but doesn’t block this time");
  });

  it("leaves out declined meetings and copies of one meeting on two calendars", async () => {
    renderDay([
      event("declined", "Skipped sync", at(12), at(12, 45), { responseStatus: "declined" }),
      event("copy-1", "All hands", at(14), at(15)),
      event("copy-2", "All hands", at(14), at(15), { calendarId: "team" }),
    ]);
    const shown = await rows();
    expect(shown).toEqual([expect.stringContaining("This time"), "30 min before All hands", expect.stringContaining("All hands")]);
  });

  it("shows the full day, all-day events first, and back", async () => {
    renderDay([
      { ...event("offsite", "Offsite", "2026-10-01", "2026-10-02"), allDay: true },
      event("early", "Breakfast", at(8), at(8, 30)),
      event("before", "Design review", at(11), at(12, 30)),
      event("late", "Gym", at(18), at(19)),
    ]);
    await rows();
    fireEvent.click(screen.getByRole("button", { name: "Show Full Day" }));
    expect(await rows()).toEqual([
      expect.stringContaining("Offsite"),
      expect.stringContaining("Breakfast"),
      expect.stringContaining("Design review"),
      "30 min after Design review",
      expect.stringContaining("This time"),
      "4 hr 30 min before Gym",
      expect.stringContaining("Gym"),
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Show Less" }));
    expect(screen.queryByText("Breakfast")).not.toBeInTheDocument();
  });

  it("says when nothing else is on the calendar that day", async () => {
    renderDay([]);
    await rows();
    expect(screen.getByText("Nothing else on your calendar that day.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show Full Day" })).not.toBeInTheDocument();
  });

  it("says why the day could not be read and can try again", async () => {
    const list = vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValueOnce(new Error("Calendar is offline."))
      .mockResolvedValueOnce({ events: [event("before", "Planning", at(11), at(12))], errors: [] });
    render(<SlotDayPreview id="day" slot={slot} timeZone={timeZone} />);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Calendar is offline.");
    fireEvent.click(within(alert).getByRole("button", { name: "Try Again" }));
    expect(await screen.findByText("Planning")).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2);
  });
});
