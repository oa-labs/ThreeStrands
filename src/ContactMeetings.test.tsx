import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ContactMeetings, MAX_UPCOMING_MEETINGS } from "./ContactMeetings";
import { clearScheduleCache } from "./calendarScheduleCache";
import { mailClient } from "./data/client";
import type { ScheduleEvent } from "./domain";
import { formatEventDate } from "./calendarTime";

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
  const view = render(<ContactMeetings people={[{ email: "Jane@Example.com", name: "Jane Doe" }, { email: "jane@work.example.com", name: "Jane Doe" }]} timeZone="UTC" onOpenEvent={onOpenEvent} />);
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
    const titles = within(section).getAllByRole("button", { name: /^Meeting/ }).map((button) => button.querySelector("strong")?.textContent);
    expect(titles).toEqual(["Meeting soon", "Meeting later"]);
    const soon = within(section).getByRole("button", { name: /Meeting soon/ });
    expect(soon).toHaveTextContent(`${formatEventDate(meeting("soon", 2, ["jane@example.com"]))} · with Jane Doe`);
    expect(soon).not.toHaveTextContent(/\d:\d\d/);
    fireEvent.click(soon);
    expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "soon" }));
  });

  it("shows the first attendee who matches anyone on the conversation", async () => {
    const onOpenEvent = vi.fn();
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({
      events: [meeting("team", 2, ["outsider@example.com", "mark@example.com", "frank@example.com"])],
      errors: [],
    });
    render(<ContactMeetings people={[{ email: "frank@example.com", name: "Frank Jackson" }, { email: "mark@example.com", name: "Mark Williams" }]} timeZone="UTC" onOpenEvent={onOpenEvent} />);
    const row = await screen.findByRole("button", { name: /Meeting team/ });
    expect(row).toHaveTextContent("with Mark Williams");
    expect(row).not.toHaveTextContent("Frank Jackson");
    fireEvent.click(row);
    expect(onOpenEvent).toHaveBeenCalledWith(expect.objectContaining({ id: "team" }));
  });

  it("shows the user's response in upcoming meetings", async () => {
    const pending = { ...meeting("pending", 2, ["jane@example.com"]), responseStatus: "needsAction" as const };
    const declined = { ...meeting("declined", 4, ["jane@example.com"]), responseStatus: "declined" as const };
    const accepted = { ...meeting("accepted", 6, ["jane@example.com"]), responseStatus: "accepted" as const };
    renderMeetings([pending, declined, accepted]);
    const section = await screen.findByRole("region", { name: "Upcoming meetings" });
    for (const [event, label] of [[pending, "Awaiting response"], [declined, "Not going"], [accepted, "Going"]] as const) {
      const row = within(section).getByRole("button", { name: new RegExp(`Meeting ${event.id}`) });
      expect(row.querySelectorAll("small")).toHaveLength(1);
      expect(row.querySelector(".context-meeting-meta")).toHaveTextContent(`${formatEventDate(event)} · with Jane Doe · ${label}`);
    }
  });

  it("shows at most the configured number of meetings", async () => {
    const count = (events: number) => Array.from({ length: events }, (_, index) => meeting(`m${index}`, index + 1, ["jane@example.com"]));
    for (const total of [MAX_UPCOMING_MEETINGS - 1, MAX_UPCOMING_MEETINGS, MAX_UPCOMING_MEETINGS + 1]) {
      clearScheduleCache();
      renderMeetings(count(total));
      const section = await screen.findByRole("region", { name: "Upcoming meetings" });
      expect(within(section).getAllByRole("button", { name: /^Meeting/ })).toHaveLength(Math.min(total, MAX_UPCOMING_MEETINGS));
      // The heading counts what the section lists, like every other panel section.
      expect(section.querySelector(".context-section-header .context-count")).toHaveTextContent(String(Math.min(total, MAX_UPCOMING_MEETINGS)));
      cleanup();
    }
  });

  it("renders nothing when there are no meetings or the schedule fails", async () => {
    const { rerender, unmount } = renderMeetings([meeting("without-jane", 2, ["bob@example.com"])]);
    // Positive control: the loaded schedule does render for an attendee it
    // includes, so the absence below is not just an unsettled first render.
    rerender(<ContactMeetings people={[{ email: "bob@example.com", name: "Bob Lee" }]} timeZone="UTC" onOpenEvent={vi.fn()} />);
    expect(await screen.findByRole("region", { name: "Upcoming meetings" })).toHaveTextContent("Meeting without-jane");
    rerender(<ContactMeetings people={[{ email: "Jane@Example.com", name: "Jane Doe" }, { email: "jane@work.example.com", name: "Jane Doe" }]} timeZone="UTC" onOpenEvent={vi.fn()} />);
    expect(screen.queryByRole("region", { name: "Upcoming meetings" })).not.toBeInTheDocument();
    unmount();

    clearScheduleCache();
    const failed = vi.spyOn(mailClient, "listScheduleEvents").mockRejectedValue(new Error("Calendar unavailable"));
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    render(<ContactMeetings people={[{ email: "jane@example.com", name: "Jane Doe" }]} timeZone="UTC" onOpenEvent={vi.fn()} />);
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
