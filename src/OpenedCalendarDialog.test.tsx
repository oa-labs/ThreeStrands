import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OpenedCalendarDialog } from "./OpenedCalendarDialog";
import { mailClient } from "./data/client";
import type { CalendarEventPreview, OpenedCalendarFile, ScheduleEvent } from "./domain";

const invitation: CalendarEventPreview = {
  uid: "launch-review@example.com",
  title: "Launch review",
  start: "2026-10-12T15:00:00Z",
  end: "2026-10-12T16:00:00Z",
  allDay: false,
  timeZone: null,
  location: "Room 4B",
  description: null,
  organizer: "Priya Shah",
  attendeeCount: 4,
  recurring: false,
  status: "CONFIRMED",
};

const onCalendar: ScheduleEvent = {
  id: "me@example.com:evt123",
  accountId: "me@example.com",
  calendarId: "me@example.com",
  title: "Launch review",
  start: "2026-10-12T15:00:00Z",
  end: "2026-10-12T16:00:00Z",
  allDay: false,
  responseStatus: "needsAction",
  canRespond: true,
};

function file(events: CalendarEventPreview[], overrides: Partial<OpenedCalendarFile> = {}): OpenedCalendarFile {
  return { name: "invite.ics", preview: { events, truncated: false }, error: null, ...overrides };
}

function renderDialog(opened: OpenedCalendarFile, options: { calendarConnected?: boolean; waiting?: number } = {}) {
  const onAddToCalendar = vi.fn();
  const onClose = vi.fn();
  render(
    <OpenedCalendarDialog
      file={opened}
      waiting={options.waiting ?? 0}
      calendarConnected={options.calendarConnected ?? true}
      onAddToCalendar={onAddToCalendar}
      onClose={onClose}
    />,
  );
  return { onAddToCalendar, onClose, dialog: screen.getByRole("dialog", { name: "invite.ics" }) };
}

describe("OpenedCalendarDialog", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows the invitation and answers the copy already on the user's calendar", async () => {
    const find = vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(onCalendar);
    const respond = vi.spyOn(mailClient, "updateCalendarResponse").mockResolvedValue({ ...onCalendar, responseStatus: "accepted" });
    const { dialog, onAddToCalendar } = renderDialog(file([invitation]));

    expect(within(dialog).getByRole("heading", { name: "Launch review" })).toBeInTheDocument();
    expect(within(dialog).getByText("Room 4B")).toBeInTheDocument();
    expect(find).toHaveBeenCalledWith("launch-review@example.com");
    expect(await within(dialog).findByText("Awaiting response")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Yes" }));

    expect(respond).toHaveBeenCalledWith(onCalendar, "accepted");
    expect(await within(dialog).findByText("Going")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true");
    expect(within(dialog).queryByRole("button", { name: /Add to Calendar/ })).toBeNull();
    expect(onAddToCalendar).not.toHaveBeenCalled();
  });

  it("says when the event is on the calendar but has no RSVP", async () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue({ ...onCalendar, responseStatus: null, canRespond: false });
    const { dialog } = renderDialog(file([invitation]));

    expect(await within(dialog).findByText("Already on your calendar · me@example.com")).toBeInTheDocument();
    expect(within(dialog).queryByRole("group", { name: "Going?" })).toBeNull();
  });

  it("offers Add to Calendar when no connected calendar has the event", async () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(null);
    const { dialog, onAddToCalendar } = renderDialog(file([invitation]));

    fireEvent.click(await within(dialog).findByRole("button", { name: "Add to Calendar" }));

    expect(onAddToCalendar).toHaveBeenCalledWith(invitation);
    expect(within(dialog).getByText("Not on your calendar yet.")).toBeInTheDocument();
  });

  it("still offers Add to Calendar, with the reason, when the calendar could not be checked", async () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockRejectedValue(new Error("Google Calendar request failed (503)."));
    const { dialog } = renderDialog(file([invitation]));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Could not check your calendar: Google Calendar request failed (503).");
    expect(within(dialog).getByRole("button", { name: "Add to Calendar" })).toBeEnabled();
  });

  it("does not look up an invitation without a UID", async () => {
    const find = vi.spyOn(mailClient, "findCalendarInvitation");
    const { dialog } = renderDialog(file([{ ...invitation, uid: "  " }]));

    expect(await within(dialog).findByRole("button", { name: "Add to Calendar" })).toBeEnabled();
    expect(find).not.toHaveBeenCalled();
  });

  it("cannot add an event without a start", () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(null);
    const { dialog } = renderDialog(file([{ ...invitation, uid: null, start: null }]));

    expect(within(dialog).getByRole("button", { name: "Add to Calendar" })).toBeDisabled();
  });

  it("does not offer to add a cancelled event", async () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(null);
    const { dialog } = renderDialog(file([{ ...invitation, status: "CANCELLED" }]));

    expect(await within(dialog).findByText("This event was cancelled.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Add to Calendar" })).toBeNull();
  });

  it("asks to connect a calendar instead of looking anything up", () => {
    const find = vi.spyOn(mailClient, "findCalendarInvitation");
    const { dialog } = renderDialog(file([invitation]), { calendarConnected: false });

    expect(within(dialog).getByText("Connect a Google Calendar account in Settings to answer or add this invitation.")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Add to Calendar" })).toBeNull();
    expect(find).not.toHaveBeenCalled();
  });

  it("explains a file that could not be read", () => {
    const { dialog } = renderDialog(file([], { preview: null, error: "This calendar file is too large to preview." }));

    expect(within(dialog).getByRole("alert")).toHaveTextContent("This calendar file is too large to preview.");
  });

  it("shows each event in a file and moves on to the next waiting file", async () => {
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(null);
    const second = { ...invitation, uid: "retro@example.com", title: "Retro" };
    const { dialog, onClose } = renderDialog(file([invitation, second]), { waiting: 2 });

    expect(within(dialog).getAllByRole("region", { name: "Calendar invitation" })).toHaveLength(2);
    expect(await within(dialog).findAllByRole("button", { name: "Add to Calendar" })).toHaveLength(2);

    fireEvent.click(within(dialog).getByRole("button", { name: "Next Invitation (2 more)" }));
    expect(onClose).toHaveBeenCalled();
  });
});
