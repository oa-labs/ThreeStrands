import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CalendarAttachment, isCalendarAttachment } from "./CalendarAttachment";
import { mailClient } from "./data/client";

const attachment = {
  id: "calendar-1",
  filename: "invite.ics",
  mimeType: "text/calendar; method=REQUEST",
  size: 512,
};

describe("CalendarAttachment", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
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
        title: "Quarterly planning",
        start: "2026-09-18T09:30:00",
        end: "2026-09-18T10:30:00",
        allDay: false,
        timeZone: "America/New_York",
        location: "Room 4B",
        description: "Review the roadmap",
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
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledWith("message-1", "calendar-1");
  });

  it("falls back to a normal attachment when parsing fails", async () => {
    vi.spyOn(mailClient, "previewCalendarAttachment").mockRejectedValue(new Error("invalid"));

    render(<CalendarAttachment messageId="message-1" attachment={attachment} onError={vi.fn()} />);

    expect(await screen.findByRole("button", { name: "View invite.ics" })).toBeVisible();
  });
});
