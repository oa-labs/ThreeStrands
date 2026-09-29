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
    const create = vi.spyOn(mailClient, "createCalendarEvent").mockResolvedValue({} as ScheduleEvent);
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
    expect(within(dialog).getByLabelText("Invite people")).toHaveValue("jane@example.com, bob@example.com");
    expect(within(dialog).getByLabelText("Description")).toHaveValue("From: Please set up the website by Friday.");
    expect(within(dialog).getByLabelText("Starts")).toHaveValue("2026-10-01T15:00");
    expect(create).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Create event" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({
      title: "Website kickoff", attendees: ["jane@example.com", "bob@example.com"], start: new Date(2026, 9, 1, 15, 0).toISOString(),
    })));
    await waitFor(() => expect(onCreated).toHaveBeenCalled());
  });
});
