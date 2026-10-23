import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InvitationSection } from "./InvitationSection";
import { mailClient } from "./data/client";
import { clearScheduleCache } from "./calendarScheduleCache";
import type { AvailabilityPreferences, CalendarEventPreview, Message, ScheduleEvent } from "./domain";

const invitation: CalendarEventPreview = {
  uid: "building-tours@example.com", title: "Building Tours", start: "2030-01-08T18:00:00Z", end: "2030-01-08T20:00:00Z",
  allDay: false, timeZone: null, location: null, description: null, organizer: "Taylor", attendeeCount: 3, recurring: false, status: "CONFIRMED",
};
const event: ScheduleEvent = {
  id: "event-1", accountId: "you@example.com", calendarId: "primary", title: "Building Tours",
  start: invitation.start!, end: invitation.end!, allDay: false, responseStatus: "needsAction", canRespond: true,
};
const message: Message = {
  id: "invite-message", threadId: "thread-1", sender: "Taylor <taylor@example.com>", recipients: ["you@example.com"],
  sentAt: "2026-10-23T12:00:00Z", bodyHtml: "", bodyText: "You are invited", unread: false,
  attachments: [{ id: "invite", filename: "invite.ics", mimeType: "text/calendar; method=REQUEST", size: 512 }],
};
const preferences: AvailabilityPreferences = {
  timeZone: "America/New_York", workingWindows: [], defaultDurationMinutes: 30, slotIncrementMinutes: 30, driveTimeBufferMinutes: 0,
};
function props() {
  return { message, calendarConnected: true, preferences, ai: false, intoDraft: false, onReplyWithTimes: vi.fn(), onOpenCalendarSettings: vi.fn() };
}

describe("InvitationSection", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "previewCalendarAttachment").mockResolvedValue({ events: [invitation], truncated: false });
    vi.spyOn(mailClient, "findCalendarInvitation").mockResolvedValue(event);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [event], errors: [] });
    vi.spyOn(mailClient, "findAvailability").mockResolvedValue({ candidates: [], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
  });
  afterEach(() => vi.restoreAllMocks());

  it.each([
    ["Accept", "accepted", "Going"], ["Maybe", "tentative", "Maybe"], ["Decline", "declined", "Not going"],
  ] as const)("answers the existing invitation with %s", async (label, status, result) => {
    const update = vi.spyOn(mailClient, "updateCalendarResponse").mockResolvedValue({ ...event, responseStatus: status });
    render(<InvitationSection {...props()} />);
    expect(screen.getByRole("status")).toHaveTextContent("Reading invitation");
    const button = await screen.findByRole("button", { name: label });
    expect(mailClient.findCalendarInvitation).toHaveBeenCalledWith(invitation.uid);
    expect(screen.getByText("Awaiting response")).toBeInTheDocument();
    fireEvent.click(button);
    await waitFor(() => expect(button).toHaveAttribute("aria-pressed", "true"));
    expect(update).toHaveBeenCalledWith(event, status);
    expect(screen.getByText(result, { selector: "strong" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
    expect(screen.queryByText("Building Tours")).not.toBeInTheDocument();
  });

  it("disables responses during an update and preserves the response after a failure", async () => {
    let reject!: (reason: Error) => void;
    vi.spyOn(mailClient, "updateCalendarResponse").mockReturnValue(new Promise((_resolve, fail) => { reject = fail; }));
    render(<InvitationSection {...props()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Accept" }));
    expect(screen.getByRole("button", { name: "Decline" })).toBeDisabled();
    await act(async () => reject(new Error("Calendar is unavailable")));
    expect(screen.getByRole("alert")).toHaveTextContent("Calendar is unavailable");
    expect(screen.getByText("Awaiting response")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Accept" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Accept" })).toHaveAttribute("aria-pressed", "false");
  });

  it("deduplicates the calendar MIME part and .ics attachment by UID", async () => {
    render(<InvitationSection {...props()} message={{ ...message, attachments: [...message.attachments, { id: "mime-part", filename: "", mimeType: "TEXT/CALENDAR; charset=utf-8", size: 200 }] }} />);
    await screen.findByRole("button", { name: "Accept" });
    expect(mailClient.previewCalendarAttachment).toHaveBeenCalledTimes(2);
    expect(mailClient.findCalendarInvitation).toHaveBeenCalledTimes(1);
    expect(screen.getAllByRole("group", { name: "Going?" })).toHaveLength(1);
  });

  it("lists real overlaps and excludes the invitation itself", async () => {
    vi.mocked(mailClient.listScheduleEvents).mockResolvedValue({ events: [event, { ...event, id: "other", title: "Josh / Joel" }, { ...event, id: "later", title: "Later", start: event.end, end: "2030-01-08T21:00:00Z" }], errors: [] });
    render(<InvitationSection {...props()} />);
    expect(await screen.findByText("Conflicts with Josh / Joel")).toBeInTheDocument();
    expect(screen.queryByText(/Conflicts with Building Tours|Conflicts with Later/)).not.toBeInTheDocument();
  });

  it("does not report a self-conflict when the invitation is the only event", async () => {
    render(<InvitationSection {...props()} />);
    await screen.findByRole("button", { name: "Accept" });
    await waitFor(() => expect(mailClient.listScheduleEvents).toHaveBeenCalled());
    expect(screen.queryByText(/Conflicts with/)).not.toBeInTheDocument();
  });

  it("uses the invitation's duration to find alternatives and passes its source to the reply draft", async () => {
    const candidate = { start: "2030-01-09T15:00:00Z", end: "2030-01-09T17:00:00Z", status: "verified" as const };
    vi.mocked(mailClient.findAvailability).mockResolvedValue({ candidates: [candidate], checkedCalendarCount: 1, totalCalendarCount: 1, uncheckedCalendars: [], errors: [] });
    const callbacks = props();
    render(<InvitationSection {...callbacks} />);
    fireEvent.click(await screen.findByRole("button", { name: "Suggest a different time" }));
    const draft = await screen.findByRole("button", { name: "Draft Reply With This Time" });
    expect(mailClient.findAvailability).toHaveBeenCalledWith(expect.objectContaining({ preferences: expect.objectContaining({ defaultDurationMinutes: 120 }) }));
    fireEvent.click(draft);
    expect(callbacks.onReplyWithTimes).toHaveBeenCalledWith([candidate], message.id);
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
  });

  it("shows a recoverable calendar lookup failure without add or response actions", async () => {
    vi.mocked(mailClient.findCalendarInvitation).mockRejectedValueOnce(new Error("Offline"));
    render(<InvitationSection {...props()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not check invitation: Offline");
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: "Accept" })).toBeEnabled();
  });

  it("can retry an unreadable attachment", async () => {
    vi.mocked(mailClient.previewCalendarAttachment).mockRejectedValueOnce(new Error("Invalid calendar file"));
    render(<InvitationSection {...props()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not read invitation: Invalid calendar file");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: "Accept" })).toBeEnabled();
  });

  it("explains an unmatched invitation without inventing an event", async () => {
    vi.mocked(mailClient.findCalendarInvitation).mockResolvedValue(null);
    render(<InvitationSection {...props()} />);
    expect(await screen.findByText("This invitation isn’t on a connected calendar yet.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Calendar" })).not.toBeInTheDocument();
  });

  it("offers calendar settings when disconnected", async () => {
    const callbacks = props();
    render(<InvitationSection {...callbacks} calendarConnected={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "Calendar Settings" }));
    expect(callbacks.onOpenCalendarSettings).toHaveBeenCalled();
    expect(mailClient.findCalendarInvitation).not.toHaveBeenCalled();
  });

  it.each([
    [{ ...invitation, status: "CANCELLED" }, "This event was cancelled."],
    [{ ...invitation, uid: "  " }, "This invitation has no event identifier, so it can’t be matched to your calendar."],
  ])("does not respond to a cancelled or unidentifiable invitation", async (preview, explanation) => {
    vi.mocked(mailClient.previewCalendarAttachment).mockResolvedValue({ events: [preview], truncated: false });
    render(<InvitationSection {...props()} />);
    expect(await screen.findByText(explanation)).toBeInTheDocument();
    expect(mailClient.findCalendarInvitation).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Suggest a different time" })).not.toBeInTheDocument();
  });

  it("keeps non-respondable events read-only", async () => {
    vi.mocked(mailClient.findCalendarInvitation).mockResolvedValue({ ...event, canRespond: false });
    render(<InvitationSection {...props()} />);
    expect(await screen.findByText("Already on your calendar. This calendar doesn’t allow you to respond.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Suggest a different time" })).not.toBeInTheDocument();
  });

  it("ignores a late lookup after switching to another invitation", async () => {
    let resolve!: (value: ScheduleEvent) => void;
    vi.mocked(mailClient.findCalendarInvitation).mockReturnValueOnce(new Promise((done) => { resolve = done; })).mockResolvedValue(null);
    const { rerender } = render(<InvitationSection {...props()} />);
    await waitFor(() => expect(mailClient.findCalendarInvitation).toHaveBeenCalled());
    rerender(<InvitationSection {...props()} message={{ ...message, id: "another-invitation" }} />);
    await screen.findByText("This invitation isn’t on a connected calendar yet.");
    await act(async () => resolve(event));
    expect(screen.queryByRole("button", { name: "Accept" })).not.toBeInTheDocument();
  });
});
