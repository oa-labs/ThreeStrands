import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CalendarAttachment, CalendarAttachmentGroup, CalendarEvent, calendarEventRange, isCalendarAttachment } from "./CalendarAttachment";
import { mailClient } from "./data/client";

const attachment = {
  id: "calendar-1",
  filename: "invite.ics",
  mimeType: "text/calendar; method=REQUEST",
  size: 512,
};

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

describe("CalendarAttachment", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.mocked(openUrl).mockClear();
  });

  it("recognizes calendar MIME types and filenames", () => {
    expect(isCalendarAttachment(attachment)).toBe(true);
    expect(isCalendarAttachment({ ...attachment, filename: "INVITE.ICS", mimeType: "application/octet-stream" })).toBe(true);
    expect(isCalendarAttachment({ ...attachment, filename: "notes.txt", mimeType: "text/plain" })).toBe(false);
  });

  it("shows parsed invitation details", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({
      truncated: false,
      events: [{
        uid: "planning@example.com",
        title: "Quarterly planning",
        start: "2026-09-18T09:30:00",
        end: "2026-09-18T10:30:00",
        allDay: false,
        timeZone: "America/New_York",
        location: "Room 4B",
        description: "<p>Review the roadmap</p><p>PIN: 812868058</p>",
        organizer: "Jane Doe",
        attendeeCount: 3,
        recurring: true,
        status: "CONFIRMED",
      }],
    });

    render(<CalendarAttachment messageId="message-1" attachment={attachment} onError={vi.fn()} />);

    expect(await screen.findByRole("heading", { name: "Quarterly planning" })).toBeVisible();
    expect(screen.getByText("Room 4B")).toBeVisible();
    expect(screen.getByText("Organized by Jane Doe · 3 attendees")).toBeVisible();
    expect(screen.getByText("Recurring event")).toBeVisible();
    expect(document.querySelector(".calendar-description")?.textContent).toBe("Review the roadmap\nPIN: 812868058");
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledWith("message-1", "calendar-1");
  });

  it("falls back to a normal attachment when parsing fails", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockRejectedValue(new Error("invalid"));

    render(<CalendarAttachment messageId="message-1" attachment={attachment} onError={vi.fn()} />);

    expect(await screen.findByRole("button", { name: "Open invite.ics" })).toBeVisible();
  });

  const event = {
    uid: null, title: "Planning", start: null, end: null, allDay: false,
    timeZone: null, location: null, description: null, organizer: null, attendeeCount: 0, recurring: false, status: null,
  };

  it("hides description separators and omits an empty cleaned description", () => {
    const separator = "-::~:~::~:~:~:~:~:~:~:~:~:~::~:~::-";
    const { container, rerender } = render(<CalendarEvent event={{ ...event, description: `${separator}\nAgenda\n${separator}` }} />);
    expect(container.querySelector(".calendar-description")?.textContent).toBe("Agenda");
    rerender(<CalendarEvent event={{ ...event, description: separator }} />);
    expect(container.querySelector(".calendar-description")).toBeNull();
  });

  it.each([
    ["https://video.example.com/planning?key=abc", "https://video.example.com/planning?key=abc"],
    ["Room 4B (http://example.org/rooms/4b), floor 2", "http://example.org/rooms/4b"],
  ])("opens a web location in the system browser: %s", (location, url) => {
    const { container } = render(<CalendarEvent event={{ ...event, location }} />);
    expect(container).toHaveTextContent(location);
    const link = screen.getByRole("link", { name: url });
    expect(link).toHaveAttribute("href", url);
    expect(openUrl).not.toHaveBeenCalled();
    fireEvent.click(link);
    expect(openUrl).toHaveBeenCalledExactlyOnceWith(url);
  });

  it.each([
    "Room 4B", "javascript:alert(1)", "data:text/html,<script>alert(1)</script>",
    "file:///tmp/invite.ics", "//example.com/meeting", "https://", '<a href="javascript:alert(1)">Join</a>',
  ])("keeps non-web locations as inert text: %s", (location) => {
    const { container } = render(<CalendarEvent event={{ ...event, location }} />);
    expect(container).toHaveTextContent(location);
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(container.querySelector("script")).toBeNull();
    expect(openUrl).not.toHaveBeenCalled();
  });
});

describe("CalendarAttachmentGroup", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("collapses attachments that describe the same event into a single card", async () => {
    const inlineIcs = { ...attachment, id: "calendar-inline" };
    const icsFile = { ...attachment, id: "calendar-file" };
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({
      truncated: false,
      events: [{
        uid: "team-lunch-20261005@calendar.example.com",
        title: "Team lunch",
        start: "2026-10-05T11:45:00",
        end: "2026-10-05T13:15:00",
        allDay: false,
        timeZone: "America/New_York",
        location: "Example Cafe, 100 Main St",
        description: null,
        organizer: "Example Organizer",
        attendeeCount: 1,
        recurring: false,
        status: "CONFIRMED",
      }],
    });

    render(<CalendarAttachmentGroup messageId="message-1" attachments={[inlineIcs, icsFile]} onError={vi.fn()} />);

    expect(await screen.findByRole("heading", { name: "Team lunch" })).toBeVisible();
    expect(screen.getAllByRole("heading", { name: "Team lunch" })).toHaveLength(1);
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledWith("message-1", "calendar-inline");
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledWith("message-1", "calendar-file");
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2);
  });

  it("renders one card per attachment when they describe different events", async () => {
    const first = { ...attachment, id: "calendar-1" };
    const second = { ...attachment, id: "calendar-2" };
    vi.spyOn(mailClient, "previewCalendarAttachment").mockImplementation((_messageId, attachmentId) =>
      Promise.resolve({
        truncated: false,
        events: [{
          uid: `${attachmentId}@calendar.example.com`,
          title: attachmentId === "calendar-1" ? "Morning standup" : "Lunch",
          start: "2026-10-05T09:00:00",
          end: "2026-10-05T09:30:00",
          allDay: false,
          timeZone: null,
          location: null,
          description: null,
          organizer: null,
          attendeeCount: 0,
          recurring: false,
          status: "CONFIRMED",
        }],
      })
    );

    render(<CalendarAttachmentGroup messageId="message-1" attachments={[first, second]} onError={vi.fn()} />);

    expect(await screen.findByRole("heading", { name: "Morning standup" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Lunch" })).toBeVisible();
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2);
  });

  it("keeps the loaded card when re-rendered with an equivalent attachment list", async () => {
    const inlineIcs = { ...attachment, id: "calendar-inline" };
    const icsFile = { ...attachment, id: "calendar-file" };
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({
      truncated: false,
      events: [{
        uid: "outlook-20261006@calendar.example.com",
        title: "Economic outlook",
        start: "2026-10-06T17:00:00",
        end: "2026-10-06T20:00:00",
        allDay: false,
        timeZone: "America/New_York",
        location: null,
        description: null,
        organizer: null,
        attendeeCount: 1,
        recurring: false,
        status: "CONFIRMED",
      }],
    });

    const { rerender } = render(
      <CalendarAttachmentGroup messageId="message-1" attachments={[inlineIcs, icsFile]} onError={vi.fn()} />,
    );
    expect(await screen.findByRole("heading", { name: "Economic outlook" })).toBeVisible();

    await act(async () => {
      rerender(
        <CalendarAttachmentGroup messageId="message-1" attachments={[{ ...inlineIcs }, { ...icsFile }]} onError={vi.fn()} />,
      );
    });

    expect(screen.queryByText("Loading calendar invitation…")).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Economic outlook" })).toBeVisible();
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2);
  });
});

describe("calendarEventRange", () => {
  const event = {
    uid: null, title: "Launch review", start: "2026-10-12T15:00:00Z", end: "2026-10-12T16:30:00Z", allDay: false,
    timeZone: null, location: null, description: null, organizer: null, attendeeCount: 0, recurring: false, status: null,
  };

  it("uses the invitation's own start and end", () => {
    expect(calendarEventRange(event)).toEqual({ start: new Date("2026-10-12T15:00:00Z"), end: new Date("2026-10-12T16:30:00Z") });
  });

  it("gives a timed event without a usable end one hour", () => {
    for (const end of [null, "2026-10-12T14:00:00Z", "not a date"]) {
      expect(calendarEventRange({ ...event, end })).toEqual({ start: new Date("2026-10-12T15:00:00Z"), end: new Date("2026-10-12T16:00:00Z") });
    }
  });

  it("reads all-day dates as local days and gives a missing end one day", () => {
    expect(calendarEventRange({ ...event, allDay: true, start: "2026-10-12", end: "2026-10-14" }))
      .toEqual({ start: new Date(2026, 9, 12), end: new Date(2026, 9, 14) });
    expect(calendarEventRange({ ...event, allDay: true, start: "2026-10-12", end: null }))
      .toEqual({ start: new Date(2026, 9, 12), end: new Date(2026, 9, 13) });
  });

  it("has no range for wall-clock times in a zone that could not be resolved", () => {
    expect(calendarEventRange({ ...event, start: "2026-10-12T15:00:00", end: "2026-10-12T16:00:00", timeZone: "Olympus Mons Time" })).toBeNull();
  });

  it("has no range without a usable start", () => {
    expect(calendarEventRange({ ...event, start: null })).toBeNull();
    expect(calendarEventRange({ ...event, start: "soon" })).toBeNull();
  });
});
