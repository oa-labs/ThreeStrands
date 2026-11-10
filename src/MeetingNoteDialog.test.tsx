import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { ContactProfile, ScheduleEvent } from "./domain";
import { MeetingNoteDialog } from "./MeetingNoteDialog";
import { expectPrimaryActionLast } from "./test/sharedButtons";

const event: ScheduleEvent = {
  id: "evt-1", accountId: "you@example.com", calendarId: "work", title: "CS Leadership Meeting (and Sales Mtg)", allDay: false,
  start: new Date(2026, 10, 12, 9, 30).toISOString(), end: new Date(2026, 10, 12, 10, 30).toISOString(),
  attendees: ["pete@example.com", "you@example.com", "mike@example.com"],
};
const pete = { id: "c-pete", displayName: "Pete B", addresses: ["pete@example.com"] } as ContactProfile;

function renderDialog(onSubmit = vi.fn(async () => undefined), onOpenNote = vi.fn()) {
  const onClose = vi.fn();
  render(<MeetingNoteDialog event={event} ownAddresses={["you@example.com"]} onClose={onClose} onSubmit={onSubmit} onOpenNote={onOpenNote} />);
  return { onSubmit, onClose, onOpenNote, dialog: within(screen.getByRole("dialog", { name: "Add Note" })) };
}

describe("MeetingNoteDialog", () => {
  beforeEach(() => {
    vi.spyOn(mailClient, "listContactProfiles").mockImplementation(async (query) => query === "pete@example.com" ? [pete] : []);
    vi.spyOn(mailClient, "resolveContactIds").mockResolvedValue({ "pete@example.com": "c-pete" });
    vi.spyOn(mailClient, "findNoteByTitle").mockResolvedValue(null);
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("fills in the event's name and other people, by name where known, and adds to the event's day", async () => {
    const { dialog, onSubmit } = renderDialog();
    expect(dialog.getByRole("textbox", { name: "Meeting name" })).toHaveValue(event.title);
    expect(dialog.getByRole("textbox", { name: "Meeting name" })).toHaveFocus();
    expect(await dialog.findByText("Pete B")).toBeInTheDocument();
    expect(dialog.getByText("mike@example.com")).toBeInTheDocument();
    expect(dialog.queryByText("you@example.com")).not.toBeInTheDocument();
    expectPrimaryActionLast(dialog.getByRole("button", { name: "Add to Daily Note" }).parentElement!);
    fireEvent.change(dialog.getByRole("textbox", { name: "Meeting name" }), { target: { value: "Leadership" } });
    fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    const [date, entry] = onSubmit.mock.calls[0]! as unknown as [string, import("./noteMeetings").MeetingEntry];
    expect(date).toBe("2026-11-12");
    expect(entry).toMatchObject({ name: "Leadership", linkNote: false, event: { kind: "event", id: "evt-1", calendar: "work" } });
    expect(entry.attendees).toEqual([
      { label: "Pete B", reference: { version: 1, kind: "contact", id: "pete@example.com", contact: "c-pete" } },
      { label: "mike@example.com", reference: { version: 1, kind: "contact", id: "mike@example.com" } },
    ]);
  });

  it("says which note a backlinked note will be, and links it", async () => {
    const find = vi.spyOn(mailClient, "findNoteByTitle").mockImplementation(async (title) => title === "Plan -Q3-" ? { id: "n1", title, dailyDate: null } : null);
    const { dialog, onSubmit } = renderDialog();
    fireEvent.click(dialog.getByRole("checkbox", { name: "Create backlinked note" }));
    expect(await dialog.findByText("Links a new note, “CS Leadership Meeting (and Sales Mtg)”.")).toBeInTheDocument();
    fireEvent.change(dialog.getByRole("textbox", { name: "Meeting name" }), { target: { value: "Plan [Q3]" } });
    expect(await dialog.findByText("Links the existing note “Plan -Q3-”.")).toBeInTheDocument();
    expect(find).toHaveBeenLastCalledWith("Plan -Q3-");
    fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith("2026-11-12", expect.objectContaining({ linkNote: true, name: "Plan [Q3]" })));
  });

  it("removes an attendee, keeps the dialog open with the reason when adding is refused, and offers the note", async () => {
    const onSubmit = vi.fn(async () => { throw new Error("That day's note changed. Open it before adding this meeting."); });
    const { dialog, onOpenNote } = renderDialog(onSubmit);
    await dialog.findByText("Pete B");
    fireEvent.click(dialog.getByRole("button", { name: /Remove mike@example\.com/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("That day's note changed.");
    expect(onSubmit).toHaveBeenCalledWith("2026-11-12", expect.objectContaining({ attendees: [expect.objectContaining({ label: "Pete B" })] }));
    expect(dialog.getByRole("button", { name: "Add to Daily Note" })).toBeEnabled();
    fireEvent.click(dialog.getByRole("button", { name: "Open Note" }));
    expect(onOpenNote).toHaveBeenCalledWith("2026-11-12");
  });

  it("won't add while an attendee is half typed, and Cancel closes", async () => {
    const { dialog, onSubmit, onClose } = renderDialog();
    fireEvent.change(dialog.getByRole("textbox", { name: "Attendees" }), { target: { value: "not finished" } });
    fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("Finish or remove");
    expect(onSubmit).not.toHaveBeenCalled();
    fireEvent.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
