import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CreateCalendarEventDialog } from "./CreateCalendarEventDialog";
import { mailClient } from "./data/client";
import type { CalendarAccount, CalendarOption, ScheduleEvent } from "./domain";

const accounts: CalendarAccount[] = [{ email: "you@example.com", connectedAt: "2026-09-01T00:00:00Z", status: "connected" }];
const calendars: CalendarOption[] = [{ id: "primary", accountId: "you@example.com", name: "You", primary: true, selected: true, writable: true }];

describe("CreateCalendarEventDialog", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

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
      onClose={vi.fn()}
      onCreated={onCreated}
    />);

    const dialog = screen.getByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("Title")).toHaveValue("Website kickoff");
    expect(within(dialog).getByRole("button", { name: "Remove jane@example.com" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Remove bob@example.com" })).toBeInTheDocument();
    expect(within(dialog).getByRole("textbox", { name: "Invite People" })).toHaveValue("");
    expect(within(dialog).getByLabelText("Description")).toHaveValue("From: Please set up the website by Friday.");
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-10-01T15:00");
    expect(create).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      title: "Website kickoff", attendees: ["jane@example.com", "bob@example.com"], start: new Date(2026, 9, 1, 15, 0).toISOString(),
    })));
    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(saved));
  });

  it("creates an all-day event with a location from a date-only suggestion", async () => {
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

    const dialog = screen.getByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("All day")).toBeChecked();
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-11-07");
    expect(within(dialog).getByLabelText("Ends")).toHaveValue("2026-11-07");
    expect(within(dialog).getByLabelText("Location")).toHaveValue("Glenn's house");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    // Google stores the day after the last day as the exclusive end.
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      start: "2026-11-07", end: "2026-11-08", allDay: true, location: "Glenn's house", attendees: [],
    })));
  });

  it("switches between all-day and timed before creating", async () => {
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({ id: "saved", accountId: accounts[0].email, title: "Planning",
      start: "2026-10-01", end: "2026-10-02", allDay: true });
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Planning" onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New event" });
    expect(within(dialog).getByLabelText("All day")).not.toBeChecked();
    fireEvent.click(within(dialog).getByLabelText("All day"));
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-10-01");
    fireEvent.change(within(dialog).getByLabelText("Ends"), { target: { value: "2026-09-30" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("The end date must be on or after the start date.");
    expect(create).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByLabelText("All day"));
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-10-01T09:00");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      allDay: false, location: "", start: new Date(2026, 9, 1, 9, 0).toISOString(), end: new Date(2026, 9, 1, 10, 0).toISOString(),
    })));
  });

  it("shows invitees as badges, commits an unfinished address on Cmd+Enter, and names invalid ones", async () => {
    const saved: ScheduleEvent = { id: "saved", accountId: accounts[0].email, calendarId: "primary", title: "Sync",
      start: new Date(2026, 9, 1, 15, 0).toISOString(), end: new Date(2026, 9, 1, 15, 30).toISOString(), allDay: false };
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue(saved);
    render(<CreateCalendarEventDialog start={new Date(2026, 9, 1, 15, 0)} end={new Date(2026, 9, 1, 15, 30)} accounts={accounts}
      calendars={calendars} initialTitle="Sync" initialInvitees={["jane@example.com"]} onClose={vi.fn()} onCreated={vi.fn()} />);
    const dialog = screen.getByRole("dialog", { name: "New event" });
    const input = within(dialog).getByRole("textbox", { name: "Invite People" });
    expect(input).toHaveAttribute("autocorrect", "off");

    fireEvent.change(input, { target: { value: "not-an-address" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(within(dialog).getByRole("button", { name: "Remove not-an-address" })).toBeInTheDocument();
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Check these invitee email addresses: not-an-address.");
    expect(create).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Remove not-an-address" }));
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: "theo@example.com" } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ attendees: ["jane@example.com", "theo@example.com"] })));
  });

  it("defaults to a visible writable calendar ahead of a hidden primary", () => {
    render(<CreateCalendarEventDialog start={new Date()} end={new Date(Date.now() + 3600000)} accounts={accounts}
      calendars={[{ ...calendars[0], selected: false }, { ...calendars[0], id: "classes", primary: false }]}
      onClose={vi.fn()} onCreated={vi.fn()} />);
    expect(screen.getByLabelText("Calendar")).toHaveValue("you@example.com\nclasses");
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
