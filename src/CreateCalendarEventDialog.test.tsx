import { changeEventDateTime, expectEventDateTime } from "./test/calendarEventFields";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CreateCalendarEventDialog, EditCalendarEventDialog } from "./CreateCalendarEventDialog";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ContactSuggestion, ScheduleEvent } from "./domain";

const accounts: CalendarAccount[] = [{ email: "you@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];
const calendars: CalendarOption[] = [{ id: "primary", accountId: "you@example.com", name: "You", primary: true, selected: true, writable: true }];
const person = (email: string, displayName: string | null, relatedTo: string[] = []): ContactSuggestion =>
  ({ email, displayName, sentCount: 0, receivedCount: 0, lastInteractedAt: "", pinned: false, relatedTo });

describe("EditCalendarEventDialog", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  const event: ScheduleEvent = {
    id: "meeting", accountId: accounts[0].email, calendarId: "primary", title: "Planning",
    start: "2026-10-01T09:00:00", end: "2026-10-01T10:30:00", allDay: false,
  };

  it("states the scope of recurring edits and saves the occurrence identity", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, seriesId: "series" }} onClose={vi.fn()} onSaved={vi.fn()} />);
    expect(screen.getByRole("note")).toHaveTextContent("This occurrence only");
    expect(screen.getByRole("note")).toHaveTextContent("Other occurrences stay unchanged.");
    fireEvent.change(screen.getByRole("textbox", { name: "Title" }), { target: { value: "Updated occurrence" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ eventId: event.id, title: "Updated occurrence" })));
  });

  it("converts displayed times when changing zones and edits in the chosen zone", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, start: "2026-11-12T01:00:00Z", end: "2026-11-12T02:30:00Z" }} onClose={vi.fn()} onSaved={vi.fn()} />);
    const zone = screen.getByRole("combobox", { name: "Time zone" });
    expect(zone).toHaveAccessibleDescription();
    fireEvent.change(zone, { target: { value: "America/Los_Angeles" } });
    expectEventDateTime(screen, "Starts", "2026-11-11T17:00");
    expectEventDateTime(screen, "Ends", "2026-11-11T18:30");
    fireEvent.change(zone, { target: { value: "Asia/Calcutta" } });
    expectEventDateTime(screen, "Starts", "2026-11-12T06:30");
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "8:50am" } });
    expectEventDateTime(screen, "Ends", "2026-11-12T10:20");
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ start: "2026-11-12T03:20:00.000Z", end: "2026-11-12T04:50:00.000Z" })));
  });

  it("keeps instants through zone changes during the repeated hour", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    const start = "2026-11-01T06:15:00.000Z";
    const end = "2026-11-01T06:45:00.000Z";
    render(<EditCalendarEventDialog event={{ ...event, start, end }} onClose={vi.fn()} onSaved={vi.fn()} />);
    const zone = screen.getByRole("combobox", { name: "Time zone" });
    fireEvent.change(zone, { target: { value: "America/New_York" } });
    expectEventDateTime(screen, "Starts", "2026-11-01T01:15");
    fireEvent.change(zone, { target: { value: "Europe/London" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ start, end })));
  });

  it("blocks times skipped by DST in the selected zone", async () => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={event} onClose={vi.fn()} onSaved={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Time zone" }), { target: { value: "America/New_York" } });
    changeEventDateTime(screen, "Starts", "2026-03-08T02:30");
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Times skipped when the clocks change cannot be used.");
    expect(update).not.toHaveBeenCalled();
  });

  it("keeps all guests while collapsed and preserves edits when hiding and reopening", async () => {
    const attendees = ["one@example.com", "two@example.com", "three@example.com", "four@example.com"];
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, attendees }} onClose={vi.fn()} onSaved={vi.fn()} />);
    expect(screen.queryByRole("textbox", { name: "Invite People" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Guests · 4 · Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove four@example.com" }));
    fireEvent.click(screen.getByRole("button", { name: "Guests · 3 · Hide" }));
    fireEvent.click(screen.getByRole("button", { name: "Guests · 3 · Edit" }));
    expect(screen.queryByRole("button", { name: "Remove four@example.com" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Guests · 3 · Hide" }));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ attendees: attendees.slice(0, 3) })));
  });

  it("saves every guest when the list stays collapsed", async () => {
    const attendees = ["one@example.com", "two@example.com", "three@example.com", "four@example.com"];
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, attendees }} onClose={vi.fn()} onSaved={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ attendees })));
  });

  it("dismisses time suggestions before dismissing the editor", () => {
    const onClose = vi.fn();
    render(<EditCalendarEventDialog event={event} onClose={onClose} onSaved={vi.fn()} />);
    fireEvent.focus(screen.getByLabelText("Start time"));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("shows no time zone for all-day events and preserves inclusive dates", () => {
    render(<EditCalendarEventDialog event={{ ...event, allDay: true, start: "2026-11-12", end: "2026-11-14" }} onClose={vi.fn()} onSaved={vi.fn()} />);
    expect(screen.queryByRole("combobox", { name: "Time zone" })).not.toBeInTheDocument();
    expectEventDateTime(screen, "Starts", "2026-11-12");
    expectEventDateTime(screen, "Ends", "2026-11-13");
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("saves a location chosen from the calendar's places", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(["Alloy 26, 100 S Commons, Pittsburgh, PA"]);
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={event} onClose={vi.fn()} onSaved={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Location" }), { target: { value: "alloy" } });
    fireEvent.mouseDown(await screen.findByRole("option", { name: "Alloy 26, 100 S Commons, Pittsburgh, PA" }));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ location: "Alloy 26, 100 S Commons, Pittsburgh, PA" })));
  });

  it("ranks Invite People suggestions by the event's account", async () => {
    const lookup = vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([person("omar@example.com", "Omar Ali")]);
    render(<EditCalendarEventDialog event={{ ...event, attendees: ["jane@example.com"] }} onClose={vi.fn()} onSaved={vi.fn()} />);
    fireEvent.change(screen.getByRole("textbox", { name: "Invite People" }), { target: { value: "omar" } });
    await screen.findByRole("option", { name: /Omar Ali/ });
    expect(lookup).toHaveBeenLastCalledWith("you@example.com", "omar", 8, ["jane@example.com"]);
  });

  it.each([
    ["a later day", event.start, event.end, "2026-10-05T09:00", "2026-10-05T10:30"],
    ["an earlier year", event.start, event.end, "2025-12-31T09:00", "2025-12-31T10:30"],
    ["a time across midnight", event.start, event.end, "2026-10-31T23:30", "2026-11-01T01:00"],
    ["a multi-day event", "2026-10-01T09:00:00", "2026-10-03T12:00:00", "2026-10-05T09:00", "2026-10-07T12:00"],
    ["spring DST", event.start, event.end, "2026-03-08T01:30", "2026-03-08T04:00"],
    ["fall DST", event.start, event.end, "2026-11-01T00:30", "2026-11-01T01:00"],
    ["an event spanning fall's repeated hour", "2026-11-01T01:15:00-04:00", "2026-11-01T01:45:00-05:00", "2026-11-02T09:00", "2026-11-02T10:30"],
  ])("preserves elapsed duration when moving the start to %s", async (_name, start, end, nextStart, nextEnd) => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, start, end }} onClose={vi.fn()} onSaved={vi.fn()} />);

    changeEventDateTime(screen, "Starts", nextStart);
    expectEventDateTime(screen, "Ends", nextEnd);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({
      start: new Date(nextStart).toISOString(),
      // Fall's 01:00 occurs twice; check the elapsed duration rather than
      // parsing that ambiguous local wall time back into a Date.
      end: new Date(new Date(nextStart).getTime() + new Date(end).getTime() - new Date(start).getTime()).toISOString(),
    })));
  });

  it.each([
    ["time", "2026-10-01T12:00", "2026-10-06T17:00"],
    ["date", "2026-10-02T10:30", "2026-10-07T15:30"],
  ])("uses an explicitly edited end %s as the new duration", async (_name, nextEnd, expectedEnd) => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={event} onClose={vi.fn()} onSaved={vi.fn()} />);
    changeEventDateTime(screen, "Ends", nextEnd);
    expectEventDateTime(screen, "Starts", "2026-10-01T09:00");
    changeEventDateTime(screen, "Starts", "2026-10-05T09:00");
    changeEventDateTime(screen, "Starts", "2026-10-06T14:00");
    expectEventDateTime(screen, "Ends", expectedEnd);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ end: new Date(expectedEnd).toISOString() })));
  });

  it.each([
    ["one day", "2026-10-01", "2026-10-02", "2026-10-05", "2026-10-05", "2026-10-06"],
    ["several days across spring DST", "2026-10-01", "2026-10-04", "2026-03-07", "2026-03-09", "2026-03-10"],
    ["several days across fall DST", "2026-03-07", "2026-03-10", "2026-10-31", "2026-11-02", "2026-11-03"],
    ["an earlier year", "2026-10-01", "2026-10-04", "2025-12-30", "2026-01-01", "2026-01-02"],
  ])("preserves %s for all-day events and saves an exclusive end", async (_name, start, end, nextStart, lastDay, exclusiveEnd) => {
    const update = vi.spyOn(mailClient, "updateCalendarEvent").mockResolvedValue(event);
    render(<EditCalendarEventDialog event={{ ...event, start, end, allDay: true }} onClose={vi.fn()} onSaved={vi.fn()} />);
    changeEventDateTime(screen, "Starts", nextStart);
    expectEventDateTime(screen, "Ends", lastDay);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ start: nextStart, end: exclusiveEnd, allDay: true })));
  });

  it("uses an edited all-day end to reset the day count, including a single day", () => {
    render(<EditCalendarEventDialog event={{ ...event, start: "2026-10-01", end: "2026-10-03", allDay: true }} onClose={vi.fn()} onSaved={vi.fn()} />);
    changeEventDateTime(screen, "Ends", "2026-10-04");
    changeEventDateTime(screen, "Starts", "2026-10-10");
    expectEventDateTime(screen, "Ends", "2026-10-13");
    changeEventDateTime(screen, "Ends", "2026-10-10");
    changeEventDateTime(screen, "Starts", "2026-10-20");
    expectEventDateTime(screen, "Ends", "2026-10-20");
  });

  it.each(["Starts", "Ends"])("retains duration while the %s field is temporarily empty", (label) => {
    render(<EditCalendarEventDialog event={event} onClose={vi.fn()} onSaved={vi.fn()} />);
    changeEventDateTime(screen, label, "");
    changeEventDateTime(screen, "Starts", "2026-10-05T09:00");
    expectEventDateTime(screen, "Ends", "2026-10-05T10:30");
  });

  it("recalculates duration when switching between all-day and timed events", () => {
    render(<EditCalendarEventDialog event={event} onClose={vi.fn()} onSaved={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("All day"));
    changeEventDateTime(screen, "Starts", "2026-10-05");
    expectEventDateTime(screen, "Ends", "2026-10-05");
    fireEvent.click(screen.getByLabelText("All day"));
    changeEventDateTime(screen, "Starts", "2026-10-06T23:30");
    expectEventDateTime(screen, "Ends", "2026-10-07T00:30");
  });
});

describe("CreateCalendarEventDialog", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("saves a location chosen from the calendar's places", async () => {
    vi.spyOn(mailClient, "searchEventLocations").mockResolvedValue(["Alloy 26, 100 S Commons, Pittsburgh, PA"]);
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({ id: "saved", accountId: accounts[0].email, title: "Cowork",
      start: new Date(2026, 9, 1, 9).toISOString(), end: new Date(2026, 9, 1, 10).toISOString(), allDay: false });
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 9)} end={new Date(2026, 9, 1, 10)} accounts={accounts} calendars={calendars}
      initialTitle="Cowork" onClose={vi.fn()} onCreated={vi.fn()} />);
    fireEvent.change(screen.getByRole("combobox", { name: "Location" }), { target: { value: "alloy" } });
    fireEvent.mouseDown(await screen.findByRole("option", { name: "Alloy 26, 100 S Commons, Pittsburgh, PA" }));
    fireEvent.click(screen.getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ location: "Alloy 26, 100 S Commons, Pittsburgh, PA" })));
  });

  it("starts from a meeting suggestion's details and still creates only on submit", async () => {
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Website kickoff",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    const onCreated = vi.fn();
    render(<CreateCalendarEventDialog
      start={new Date(2026, 9, 1, 15, 0)}
      end={new Date(2026, 9, 1, 15, 30)}
      accounts={accounts}
      calendars={calendars}
      initialTitle="Website kickoff"
      initialInvitees={["jane@example.com", "bob@example.com"]}
      initialDescription="From: Please set up the website by Friday."
      sourceMessageId="message-1"
      onClose={vi.fn()}
      onCreated={onCreated}
    />);

    const dialog = screen.getByRole("dialog", { name: "New Event" });
    expect(within(dialog).getByLabelText("Title")).toHaveValue("Website kickoff");
    expect(within(dialog).getByRole("button", { name: "Remove jane@example.com" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Remove bob@example.com" })).toBeInTheDocument();
    expect(within(dialog).getByRole("textbox", { name: "Invite People" })).toHaveValue("");
    expect(within(dialog).getByLabelText("Description")).toHaveValue("From: Please set up the website by Friday.");
    expectEventDateTime(within(dialog), "Starts", "2026-10-01T15:00");
    expect(create).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      title: "Website kickoff", attendees: ["jane@example.com", "bob@example.com"], start: new Date(2026, 9, 1, 15, 0).toISOString(),
      // The email is tagged on the event so the Brief can recognize it there later.
      sourceMessageId: "message-1",
    })));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(saved));
  });

  it("keeps a date-only suggestion timed, preserves its dates, and requires both times", async () => {
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, title: "Lunch",
      start: new Date(2026, 10, 7, 12).toISOString(), end: new Date(2026, 10, 7, 13).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog start={new Date(2026, 10, 7)} end={new Date(2026, 10, 7)}
      accounts={accounts} calendars={calendars} initialTitle="Lunch" initialTimeUnspecified
      onClose={vi.fn()} onCreated={vi.fn()} />);
    expect(screen.getByLabelText("All day")).not.toBeChecked();
    expect(screen.getByLabelText("Start date")).toHaveValue("2026-11-07");
    expect(screen.getByLabelText("End date")).toHaveValue("2026-11-07");
    expect(screen.getByLabelText("Start time")).toHaveValue("");
    expect(screen.getByLabelText("End time")).toHaveValue("");
    // Submitting directly also exercises validation beyond the required inputs.
    const form = screen.getByRole("button", { name: "Create event" }).closest("form")!;
    fireEvent.submit(form);
    expect(await screen.findByRole("alert")).toHaveTextContent("Enter a start and end time");
    expect(create).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "12:00" } });
    fireEvent.submit(form);
    expect(create).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("End time"), { target: { value: "11:00" } });
    fireEvent.submit(form);
    expect(await screen.findByRole("alert")).toHaveTextContent("The end time must be after the start time.");
    expect(create).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("End time"), { target: { value: "13:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      allDay: false, start: saved.start, end: saved.end,
    })));
  });

  it("lets the user explicitly choose all day for an unknown time without inventing times when switching back", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({ id: "saved", accountId: accounts[0].email,
      title: "Lunch", start: "2026-11-07", end: "2026-11-08", allDay: true });
    render(<CreateCalendarEventDialog start={new Date(2026, 10, 7)} end={new Date(2026, 10, 7)}
      accounts={accounts} calendars={calendars} initialTitle="Lunch" initialTimeUnspecified
      onClose={vi.fn()} onCreated={vi.fn()} />);
    fireEvent.click(screen.getByLabelText("All day"));
    expectEventDateTime(screen, "Starts", "2026-11-07");
    fireEvent.click(screen.getByLabelText("All day"));
    expect(screen.getByLabelText("Start time")).toHaveValue("");
    expect(screen.getByLabelText("End time")).toHaveValue("");
    fireEvent.click(screen.getByLabelText("All day"));
    fireEvent.click(screen.getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      allDay: true, start: "2026-11-07", end: "2026-11-08",
    })));
  });

  it("creates an explicitly all-day event with a location", async () => {
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Girls group bonfire",
      start: "2026-11-07", end: "2026-11-08", allDay: true };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog
      start={new Date(2026, 10, 7)}
      end={new Date(2026, 10, 7)}
      accounts={accounts}
      calendars={calendars}
      initialTitle="Girls group bonfire"
      initialLocation="Glenn's house"
      initialAllDay
      onClose={vi.fn()}
      onCreated={vi.fn()}
    />);

    const dialog = screen.getByRole("dialog", { name: "New Event" });
    expect(within(dialog).getByLabelText("All day")).toBeChecked();
    expectEventDateTime(within(dialog), "Starts", "2026-11-07");
    expectEventDateTime(within(dialog), "Ends", "2026-11-07");
    expect(within(dialog).getByLabelText("Location")).toHaveValue("Glenn's house");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    // Google stores the day after the last day as the exclusive end.
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      start: "2026-11-07", end: "2026-11-08", allDay: true, location: "Glenn's house", attendees: [],
    })));
    expect(create.mock.calls[0][0]).not.toHaveProperty("sourceMessageId");
  });

  it("switches between all-day and timed before creating", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({ id: "saved", accountId: accounts[0].email, title: "Planning",
      start: "2026-10-01", end: "2026-10-02", allDay: true });
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Planning" onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Event" });
    expect(within(dialog).getByLabelText("All day")).not.toBeChecked();
    fireEvent.click(within(dialog).getByLabelText("All day"));
    expectEventDateTime(within(dialog), "Starts", "2026-10-01");
    changeEventDateTime(within(dialog), "Ends", "2026-09-30");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The end date must be on or after the start date.");
    expect(create).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByLabelText("All day"));
    expectEventDateTime(within(dialog), "Starts", "2026-10-01T09:00");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      allDay: false, location: "", start: new Date(2026, 9, 1, 9, 0).toISOString(), end: new Date(2026, 9, 1, 10, 0).toISOString(),
    })));
  });

  it("shows invitees as badges, commits an unfinished address on Cmd+Enter, and keeps text that isn't an address out of the badges", async () => {
    vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([]);
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Sync",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Sync" initialInvitees={["jane@example.com"]} onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Event" });
    const input = within(dialog).getByRole("textbox", { name: "Invite People" });
    expect(input).toHaveAttribute("autocorrect", "off");

    fireEvent.change(input, { target: { value: "not-an-address" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(within(dialog).queryByRole("button", { name: "Remove not-an-address" })).not.toBeInTheDocument();
    expect(input).toHaveValue("not-an-address");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(within(dialog).getByRole("alert")).toHaveTextContent("“not-an-address” isn't an email address. Choose a contact or type a full address.");

    // Saving stops at the field instead.
    fireEvent.click(within(dialog).getByLabelText("Title"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(input).toHaveFocus());
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    expect(create).not.toHaveBeenCalled();

    // Editing clears the hint until the next commit.
    fireEvent.change(input, { target: { value: "theo@example.com" } });
    expect(input).not.toHaveAttribute("aria-invalid");
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ attendees: ["jane@example.com", "theo@example.com"] })));
  });

  it("adds the addresses in pasted or blurred text and leaves the rest to fix", () => {
    vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([]);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} onClose={vi.fn()} onCreated={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Invite People" });

    fireEvent.paste(input, { clipboardData: { getData: () => 'omar@example.com; "Doe, Jane" <jane@example.com>, bob carl@example' } });
    expect(screen.getByRole("button", { name: "Remove omar@example.com" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove Doe, Jane" })).toBeInTheDocument();
    expect(input).toHaveValue("bob carl@example");
    expect(screen.getByRole("alert")).toHaveTextContent("“bob carl@example” isn't an email address.");

    // A name being typed is kept whole when the field loses focus.
    fireEvent.change(input, { target: { value: "Jane Doe" } });
    fireEvent.blur(input);
    expect(input).toHaveValue("Jane Doe");
    expect(screen.queryByRole("button", { name: "Remove Jane" })).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("“Jane Doe” isn't an email address.");
  });

  it("marks a suggested invitee that isn't an address and refuses to create until it's removed", async () => {
    vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([]);
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Sync",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Sync" initialInvitees={["jane@example.com", "Bob"]} onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Event" });
    expect(within(dialog).getByText("Bob").closest(".recipient-chip")).toHaveClass("invalid");
    expect(within(dialog).getByText("jane@example.com").closest(".recipient-chip")).not.toHaveClass("invalid");
    expect(within(dialog).getByRole("alert")).toHaveTextContent("“Bob” isn't an email address. Remove it or type a full address.");

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    expect(await within(dialog).findByText("Check these invitee email addresses: Bob.")).toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Remove Bob" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ attendees: ["jane@example.com"] })));
  });

  it("suggests people for Invite People and adds the chosen one as a named badge", async () => {
    const lookup = vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([
      person("jane@example.com", "Jane Doe"),
      person("jane.doe@work.example", "Jane Doe"),
      person("janet@example.com", null),
    ]);
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Sync",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    const onClose = vi.fn();
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Sync" initialInvitees={["jane@example.com"]} onClose={onClose} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Event" });
    const input = within(dialog).getByRole("textbox", { name: "Invite People" });

    // A space continues a name being looked up rather than committing it.
    fireEvent.change(input, { target: { value: "Jane" } });
    fireEvent.keyDown(input, { key: " " });
    expect(within(dialog).queryByRole("button", { name: "Remove Jane" })).not.toBeInTheDocument();
    fireEvent.change(input, { target: { value: "Jane D" } });
    const list = await within(dialog).findByRole("listbox", { name: "Invite People suggestions" });
    // The event's calendar account ranks the people, with those already invited as context.
    expect(lookup).toHaveBeenLastCalledWith("you@example.com", "Jane D", 8, ["jane@example.com"]);
    // An address already invited is not suggested again.
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "Jane Doejane.doe@work.example", "janet@example.com",
    ]);
    expect(input).toHaveAttribute("aria-expanded", "true");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(within(list).getByRole("option", { name: "janet@example.com" })).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(within(dialog).getByRole("button", { name: "Remove Jane Doe" })).toBeInTheDocument();
    expect(within(dialog).getByText("Jane Doe")).toHaveAttribute("title", "jane.doe@work.example");
    expect(input).toHaveValue("");
    expect(within(dialog).queryByRole("listbox")).not.toBeInTheDocument();

    fireEvent.change(input, { target: { value: "jan" } });
    const next = await within(dialog).findByRole("listbox", { name: "Invite People suggestions" });
    // Escape closes the suggestions, not the dialog.
    fireEvent.keyDown(input, { key: "Escape" });
    expect(next).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "jane" } });
    fireEvent.mouseDown(within(await within(dialog).findByRole("listbox")).getByRole("option", { name: "janet@example.com" }));
    expect(within(dialog).getByRole("button", { name: "Remove janet@example.com" })).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      attendees: ["jane@example.com", "jane.doe@work.example", "janet@example.com"],
    })));
  });

  it("suggests contact groups ahead of contacts and invites every member not already invited", async () => {
    vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([
      { id: "board", name: "Board", members: [
        { contactId: "jane", email: "jane@example.com", displayName: "Jane Doe", addresses: ["jane@example.com"] },
        { contactId: "omar", email: "omar@example.com", displayName: "Omar Ali", addresses: ["omar@example.com"] },
        { contactId: "lee", email: "lee@example.com", displayName: null, addresses: ["lee@example.com"] },
      ] },
      { id: "empty", name: "Boardgames", members: [] },
    ]);
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Sync",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Sync" initialInvitees={["jane@example.com"]} onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New Event" });
    const input = within(dialog).getByRole("textbox", { name: "Invite People" });

    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "boa" } });
    const list = await within(dialog).findByRole("listbox", { name: "Invite People suggestions" });
    // A group with no members is not offered.
    expect(within(list).getAllByRole("option").map((option) => option.getAttribute("aria-label"))).toEqual(["Board, group of 3 members"]);
    fireEvent.keyDown(input, { key: "Enter" });

    expect(within(dialog).getByRole("button", { name: "Remove Omar Ali" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Remove lee@example.com" })).toBeInTheDocument();
    expect(within(dialog).getByRole("status")).toHaveTextContent("Added 2 people from Board · 1 already in Invite People");
    expect(input).toHaveValue("");

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      attendees: ["jane@example.com", "omar@example.com", "lee@example.com"],
    })));
  });

  it("offers who is often emailed with the invitees as soon as Invite People is focused", async () => {
    const lookup = vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([
      person("omar@example.com", "Omar Ali", ["jane@example.com"]),
    ]);
    vi.spyOn(mailClient, "listContactGroupRecipients").mockResolvedValue([]);
    const work: CalendarOption = { ...calendars[0], id: "team", accountId: "work@example.com", name: "Team", primary: false };
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)}
      accounts={[...accounts, { ...accounts[0], email: "work@example.com" }]} calendars={[...calendars, work]}
      initialInvitees={["jane@example.com"]} onClose={vi.fn()} onCreated={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Invite People" });

    fireEvent.focus(input);
    const option = within(await screen.findByRole("listbox", { name: "Invite People suggestions" })).getByRole("option");
    expect(lookup).toHaveBeenLastCalledWith("you@example.com", "", 8, ["jane@example.com"]);
    expect(option).toHaveTextContent("Often emailed with jane@example.com");

    // Choosing them refreshes the suggestions with both people as context.
    fireEvent.mouseDown(option);
    await waitFor(() => expect(lookup).toHaveBeenLastCalledWith("you@example.com", "", 8, ["jane@example.com", "omar@example.com"]));

    // A different calendar's account ranks the people instead.
    fireEvent.change(screen.getByLabelText("Calendar"), { target: { value: "work@example.com\nteam" } });
    fireEvent.focus(input);
    await waitFor(() => expect(lookup).toHaveBeenLastCalledWith("work@example.com", "", 8, ["jane@example.com", "omar@example.com"]));
  });

  it("commits a typed address on space while suggestions are available", async () => {
    vi.spyOn(mailClient, "listInviteeSuggestions").mockResolvedValue([]);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} onClose={vi.fn()} onCreated={vi.fn()} />);
    const input = screen.getByRole("textbox", { name: "Invite People" });
    fireEvent.change(input, { target: { value: "theo@example.com" } });
    fireEvent.keyDown(input, { key: " " });
    expect(screen.getByRole("button", { name: "Remove theo@example.com" })).toBeInTheDocument();
    expect(input).toHaveValue("");
  });

  it("defaults to a visible writable calendar ahead of a hidden primary", () => {
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={accounts}
      calendars={[{ ...calendars[0], selected: false }, { ...calendars[0], id: "classes", primary: false }]}
      onClose={vi.fn()} onCreated={vi.fn()} />);
    expect(screen.getByLabelText("Calendar")).toHaveValue("you@example.com\nclasses");
  });

  it("defaults to the preferred account's calendar and lists each account's primary calendar first", () => {
    const twoAccounts: CalendarAccount[] = [...accounts, { email: "work@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={twoAccounts}
      calendars={[
        { ...calendars[0], id: "family", name: "Family", primary: false },
        calendars[0],
        { id: "team", accountId: "work@example.com", name: "Team", primary: false, selected: true, writable: true },
        { id: "work", accountId: "work@example.com", name: "Work", primary: true, selected: true, writable: true },
      ]}
      preferredAccountId="work@example.com" onClose={vi.fn()} onCreated={vi.fn()} />);
    const select = screen.getByLabelText("Calendar");
    expect(select).toHaveValue("work@example.com\nwork");
    const groups = within(select).getAllByRole("group");
    expect(groups.map((group) => group.getAttribute("label"))).toEqual(["Primary calendars", "Other calendars"]);
    expect(within(groups[0]).getAllByRole("option").map((option) => option.textContent)).toEqual(["You · you@example.com", "Work · work@example.com"]);
    expect(within(groups[1]).getAllByRole("option").map((option) => option.textContent)).toEqual(["Family · you@example.com", "Team · work@example.com"]);
  });

  it("lists calendars without headings when none or all are primary", () => {
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={accounts}
      calendars={calendars} onClose={vi.fn()} onCreated={vi.fn()} />);
    expect(within(screen.getByLabelText("Calendar")).queryByRole("group")).not.toBeInTheDocument();
  });

  it("shows an explicitly chosen hidden calendar before creating, preserving the account's other selections", async () => {
    let finishSelection!: () => void;
    const select = vi.spyOn(mailClient, "setCalendarSelection").mockImplementation(() => new Promise((resolve) => {
      finishSelection = () => resolve([]);
    }));
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({} as ScheduleEvent);
    const onCreated = vi.fn();
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={accounts}
      calendars={[...calendars, { ...calendars[0], id: "classes", primary: false, selected: false },
        { ...calendars[0], id: "other-account", accountId: "someone@example.com" }]}
      initialTitle="Class" onClose={vi.fn()} onCreated={onCreated} />);
    fireEvent.change(screen.getByLabelText("Calendar"), { target: { value: "you@example.com\nclasses" } });
    expect(screen.getByText("This calendar will be shown in your schedule.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Create event" }));
    expect(select).toHaveBeenCalledWith("you@example.com", ["primary", "classes"]);
    expect(create).not.toHaveBeenCalled();
    finishSelection();
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ calendarId: "classes" })));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
  });

  it("keeps the dialog open without creating an event when making its calendar visible fails", async () => {
    vi.spyOn(mailClient, "setCalendarSelection").mockRejectedValue(new Error("Calendar selection failed"));
    const create = vi.spyOn(mailClient, "createCalendarEvent");
    const onCreated = vi.fn();
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={accounts}
      calendars={[{ ...calendars[0], selected: false }]} initialTitle="Class" onClose={vi.fn()} onCreated={onCreated} />);
    fireEvent.click(screen.getByRole("button", { name: "Create event" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Calendar selection failed");
    expect(create).not.toHaveBeenCalled();
    expect(onCreated).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Create event" })).toBeEnabled();
  });
});
