import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ContactMeetings, MAX_UPCOMING_MEETINGS } from "./ContactMeetings";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";

const hour = 60 * 60 * 1000;

function meeting(id: string, startOffsetHours: number, attendees: string[] | undefined): ScheduleEvent {
  const start = new Date(Date.now() + startOffsetHours * hour);
  return {
    id, accountId: "you@example.com", title: `Meeting ${id}`, allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + hour / 2).toISOString(), attendees,
  };
}

function renderMeetings(events: ScheduleEvent[], errors: string[] = []) {
  const listScheduleEvents = vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events, errors });
  const onOpenEvent = vi.fn();
  const view = render(<ContactMeetings addresses={["Jane@Example.com", "jane@work.example.com"]} timeZone="UTC" onOpenEvent={onOpenEvent} />);
  return { ...view, onOpenEvent, listScheduleEvents };
}

describe("ContactMeetings", () => {
  beforeEach(clearScheduleCache);
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("lists upcoming events that include any of the person's addresses, soonest first", async () => {
    const { onOpenEvent } = renderMeetings([
      meeting("later", 48, ["jane@work.example.com"]),
      meeting("soon", 2, ["bob@example.com", "jane@example.com"]),
      meeting("ended", -3, ["jane@example.com"]),
      meeting("without-jane", 5, ["bob@example.com"]),
      meeting("unknown-attendees", 6, undefined),
    ]);

    const section = await screen.findByRole("region", { name: "Upcoming meetings" });
    const titles = within(section).getAllByRole("button").map((button) => button.querySelector("strong")?.textContent);
    expect(titles).toEqual(["Meeting soon", "Meeting later"]);
    fireEvent.click(within(section).getByRole("button", { name: /Meeting soon/ }));
    expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "soon" }));
  });

  it("shows at most the configured number of meetings", async () => {
    const count = (events: number) => Array.from({ length: events }, (_, index) => meeting(`m${index}`, index + 1, ["jane@example.com"]));
    for (const total of [MAX_UPCOMING_MEETINGS - 1, MAX_UPCOMING_MEETINGS, MAX_UPCOMING_MEETINGS + 1]) {
      clearScheduleCache();
      renderMeetings(count(total));
      const section = await screen.findByRole("region", { name: "Upcoming meetings" });
      expect(within(section).getAllByRole("button")).toHaveLength(Math.min(total, MAX_UPCOMING_MEETINGS));
      cleanup();
    }
  });

  it("renders nothing when there are no meetings or the schedule fails", async () => {
    const { rerender, unmount } = renderMeetings([meeting("without-jane", 2, ["bob@example.com"])]);
    // Positive control: the loaded schedule does render for an attendee it
    // includes, so the absence below is not just an unsettled first render.
    rerender(<ContactMeetings addresses={["bob@example.com"]} timeZone="UTC" onOpenEvent={vi.fn()} />);
    expect(await screen.findByRole("region", { name: "Upcoming meetings" })).toHaveTextContent("Meeting without-jane");
    rerender(<ContactMeetings addresses={["Jane@Example.com", "jane@work.example.com"]} timeZone="UTC" onOpenEvent={vi.fn()} />);
    expect(screen.queryByRole("region", { name: "Upcoming meetings" })).not.toBeInTheDocument();
    unmount();

    clearScheduleCache();
    const failed = vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValue(new Error("Calendar unavailable"));
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<ContactMeetings addresses={["jane@example.com"]} timeZone="UTC" onOpenEvent={vi.fn()} />);
    // Wait until the hook has handled the rejection, not merely issued the call.
    await waitFor(() => expect(error).toHaveBeenCalledWith("Calendar schedule load failed:", expect.objectContaining({ message: "Calendar unavailable" })));
    expect(failed).toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: "Upcoming meetings" })).not.toBeInTheDocument();
  });

  it("still shows meetings from calendars that loaded when another calendar fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    renderMeetings([meeting("soon", 2, ["jane@example.com"])], ["team@example.com: Calendar unavailable"]);
    expect(await screen.findByRole("region", { name: "Upcoming meetings" })).toHaveTextContent("Meeting soon");
  });
});
