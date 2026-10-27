import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeetingAvailabilityPicker } from "./MeetingAvailabilityPicker";
import { EditCalendarEventDialog } from "./CreateCalendarEventDialog";
import { mailClient } from "./data/client";
import type { MeetingAvailabilityResult, ScheduleEvent } from "./domain";
import { DEFAULT_AVAILABILITY_PREFERENCES, saveAvailabilityPreferences } from "./settings";

const event: ScheduleEvent = { id: "event", accountId: "me@example.com", calendarId: "primary", canEdit: true,
  title: "Planning", start: "2099-09-21T13:00:00Z", end: "2099-09-21T13:30:00Z", allDay: false,
  attendees: ["alex@example.com", "sam@example.com"], description: "Agenda", location: "Meeting room" };
const result: MeetingAvailabilityResult = {
  own: { email: "me@example.com", checked: true, busy: [{ start: event.start, end: event.end }] },
  guests: [
    { email: "alex@example.com", checked: true, busy: [{ start: "2099-09-21T14:00:00Z", end: "2099-09-21T15:00:00Z" }] },
    { email: "sam@example.com", checked: false, busy: [] },
  ],
  candidates: [{ start: "2099-09-21T15:00:00Z", end: "2099-09-21T15:30:00Z", status: "partiallyChecked" }],
};
function picker(onSelect = vi.fn()) {
  return <MeetingAvailabilityPicker eventId={event.id} accountId={event.accountId} calendarId="primary" attendees={event.attendees!}
    originalStart={event.start} originalEnd={event.end} start={Date.parse(event.start)} end={Date.parse(event.end)} disabled={false} onSelect={onSelect} />;
}

describe("meeting rescheduling", () => {
  beforeEach(() => {
    saveAvailabilityPreferences({ ...DEFAULT_AVAILABILITY_PREFERENCES, timeZone: "America/New_York" });
    vi.spyOn(mailClient, "meetingAvailability").mockResolvedValue(structuredClone(result));
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

  it("shows individual busy lanes, unknown coverage, and suggestions without saving", async () => {
    const select = vi.fn();
    const update = vi.spyOn(mailClient, "updateCalendarEvent");
    render(picker(select));
    expect(await screen.findByRole("img", { name: /alex@example.com: Busy/ })).toBeVisible();
    expect(screen.getByText(/Availability unavailable for sam@example.com/)).toBeVisible();
    expect(screen.getByText(/stays marked busy/)).toBeVisible();
    expect(screen.getByText(/America\/New_York/)).toBeVisible();
    const suggestions = screen.getByRole("complementary", { name: "Suggested times" });
    fireEvent.click(within(suggestions).getByRole("button"));
    expect(select).toHaveBeenCalledWith(Date.parse(result.candidates[0].start), Date.parse(result.candidates[0].end));
    expect(update).not.toHaveBeenCalled();
    expect(mailClient.meetingAvailability).toHaveBeenCalledWith(expect.objectContaining({
      eventId: "event", accountId: event.accountId, calendarId: "primary", attendees: ["alex@example.com", "sam@example.com"],
      preferences: expect.objectContaining({ defaultDurationMinutes: 30, timeZone: "America/New_York" }),
    }));
  });

  it("allows keyboard-accessible manual slot selection, preserving duration", async () => {
    const select = vi.fn();
    render(picker(select));
    await screen.findByRole("img", { name: /alex@example.com: Busy/ });
    const button = screen.getAllByRole("button", { name: /^Select / })[40];
    button.focus();
    expect(button).toHaveFocus();
    fireEvent.click(button);
    const [start, end] = select.mock.calls[0];
    expect(end - start).toBe(30 * 60_000);
    expect(new Date(start).toISOString()).toBe("2099-09-21T14:00:00.000Z");
  });

  it("ignores late responses from a previous week", async () => {
    let resolveFirst!: (value: MeetingAvailabilityResult) => void;
    vi.mocked(mailClient.meetingAvailability).mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }));
    render(picker());
    fireEvent.click(screen.getByRole("button", { name: "Next week" }));
    await screen.findByRole("complementary", { name: "Suggested times" });
    resolveFirst({ ...result, guests: [{ email: "stale@example.com", checked: false, busy: [] }] });
    await waitFor(() => expect(screen.queryByText("stale@example.com")).not.toBeInTheDocument());
  });

  it("offers retry after a failed availability request", async () => {
    vi.mocked(mailClient.meetingAvailability).mockRejectedValueOnce(new Error("Offline"));
    render(picker());
    expect(await screen.findByRole("alert")).toHaveTextContent("Offline");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("complementary", { name: "Suggested times" })).toBeVisible();
  });

  it("updates editor times without discarding unsaved details, then refreshes before notifying guests", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    const onSaved = vi.fn();
    render(<EditCalendarEventDialog event={event} initialFindingTime onClose={vi.fn()} onSaved={onSaved} />);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Updated planning" } });
    fireEvent.change(screen.getByLabelText("Description"), { target: { value: "New agenda" } });
    fireEvent.click(within(await screen.findByRole("complementary", { name: "Suggested times" })).getByRole("button"));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("choose Save changes again");
    expect(update).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(event));
    expect(update).toHaveBeenCalledWith(expect.objectContaining({ title: "Updated planning", description: "New agenda", location: "Meeting room",
      start: result.candidates[0].start.replace("Z", ".000Z"), end: result.candidates[0].end.replace("Z", ".000Z"), attendees: event.attendees }));
    expect(vi.mocked(mailClient.meetingAvailability).mock.calls.at(-1)?.[0]).toMatchObject({ rangeStart: result.candidates[0].start.replace("Z", ".000Z"), rangeEnd: result.candidates[0].end.replace("Z", ".000Z") });
  });

  it("blocks saving when the fresh availability check fails", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent");
    render(<EditCalendarEventDialog event={event} initialFindingTime onClose={vi.fn()} onSaved={vi.fn()} />);
    await screen.findByRole("complementary", { name: "Suggested times" });
    vi.mocked(mailClient.meetingAvailability).mockRejectedValueOnce(new Error("Check failed"));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Check failed");
    expect(update).not.toHaveBeenCalled();
  });

  it("saves a fully checked free time once and catches a newly introduced conflict", async () => {
    const free = { own: { ...result.own, busy: [] }, guests: result.guests.map((guest) => ({ ...guest, checked: true, busy: [] })), candidates: result.candidates };
    vi.mocked(mailClient.meetingAvailability).mockResolvedValue(free);
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    const saved = vi.fn();
    const view = render(<EditCalendarEventDialog event={event} initialFindingTime onClose={vi.fn()} onSaved={saved} />);
    await screen.findByRole("complementary", { name: "Suggested times" });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(saved).toHaveBeenCalledOnce());
    view.unmount(); update.mockClear();
    render(<EditCalendarEventDialog event={event} initialFindingTime onClose={vi.fn()} onSaved={vi.fn()} />);
    await screen.findByRole("complementary", { name: "Suggested times" });
    vi.mocked(mailClient.meetingAvailability).mockResolvedValueOnce({ ...free, guests: [{ email: "alex@example.com", checked: true, busy: [{ start: event.start, end: event.end }] }] });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("overlaps alex@example.com");
    expect(update).not.toHaveBeenCalled();
  });

  it("rechecks when invitees change and preserves editor details while hiding the picker", async () => {
    render(<EditCalendarEventDialog event={event} initialFindingTime onClose={vi.fn()} onSaved={vi.fn()} />);
    await screen.findByRole("complementary", { name: "Suggested times" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Draft title" } });
    const invitees = screen.getByRole("textbox", { name: "Invite People" });
    fireEvent.change(invitees, { target: { value: "new@example.com" } });
    fireEvent.keyDown(invitees, { key: "Enter" });
    await waitFor(() => expect(vi.mocked(mailClient.meetingAvailability).mock.calls.at(-1)?.[0].attendees).toContain("new@example.com"));
    fireEvent.click(screen.getByRole("button", { name: "Hide availability" }));
    expect(screen.getByLabelText("Title")).toHaveValue("Draft title");
    expect(screen.queryByRole("complementary", { name: "Suggested times" })).not.toBeInTheDocument();
    const beforeSave = vi.mocked(mailClient.meetingAvailability).mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(mailClient.meetingAvailability).toHaveBeenCalledTimes(beforeSave + 1));
  });

  it("keeps all-day edits out of the timed availability picker", () => {
    render(<EditCalendarEventDialog event={{ ...event, allDay: true, start: "2099-09-21", end: "2099-09-22" }} onClose={vi.fn()} onSaved={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Find a time" })).not.toBeInTheDocument();
    expect(mailClient.meetingAvailability).not.toHaveBeenCalled();
  });
});
