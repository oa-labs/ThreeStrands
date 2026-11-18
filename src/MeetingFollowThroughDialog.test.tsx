import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { DraftHandoff, FollowThroughSave, MeetingContext, MeetingSection, ScheduleEvent } from "./domain";
import { MeetingFollowThroughDialog, MeetingFollowUpDialog } from "./MeetingFollowThroughDialog";
import { readFollowThroughRecovery, readFollowUpHandoff, rememberFollowUpHandoff, saveFollowThroughRecovery } from "./meetingFollowThrough";
import { meetingEventReference } from "./meetingNoteEntry";
import { expectPrimaryActionLast } from "./test/sharedButtons";

const ME = "me@example.com";
const event: ScheduleEvent = {
  id: "evt-1", accountId: ME, calendarId: "work", title: "Sync", allDay: false,
  start: "2026-11-16T09:00:00", end: "2026-11-16T09:30:00", attendees: ["ann@example.org", ME, "cal@example.com"],
};
const reference = meetingEventReference(event)!;
const section = <T,>(items: T[], extra: Partial<MeetingSection<T>> = {}): MeetingSection<T> => ({ status: "ready", items, omitted: 0, message: null, ...extra });
const DAILY = { noteId: "daily:2026-11-16", title: "2026-11-16", dailyDate: "2026-11-16", revision: "r1", kind: "excerpt" as const, text: "- [9am](x) for Sync\n  - Ship on Friday\n  - ( ) Send the deck <!-- threestrands-task:abc -->", truncated: false, start: 0, end: 10 };
const REUSABLE = { noteId: "n-sync", title: "Sync notes", dailyDate: null, revision: "r9", kind: "linkedNote" as const, text: "Running notes", truncated: false, start: 0, end: 13 };

function context(overrides: Partial<MeetingContext> = {}): MeetingContext {
  return {
    reference,
    event: { title: "Sync", start: event.start, end: event.end, allDay: false, seriesId: null, attendees: ["ann@example.org", "cal@example.com"], attendeesOmitted: 0, sourceMessageId: null, responseStatus: null },
    mailScope: [ME],
    thisMeeting: section([DAILY, REUSABLE]),
    previousMeetings: section([]), seriesNotes: section([]),
    conversations: section([{ threadId: "t1", accountId: ME, subject: "Sync agenda", lastMessageAt: "2026-11-14T09:00:00Z", reason: "title", attendees: [], related: true }]),
    commitments: section([{ id: "task-9", title: "Review the contract", status: "open", noteId: null, threadId: "t9", accountId: ME, origin: "originConversation", candidate: false }]),
    followThrough: section([]),
    limits: { noteSources: 20, priorOccurrences: 8, excerptBytes: 4096, linkedNotePreviewBytes: 1024, attendees: 50, conversations: 10, mailLookbackDays: 90, commitments: 25 },
    ...overrides,
  };
}

const saved = (operationId: string): FollowThroughSave => ({
  operationId, note: { id: DAILY.noteId, title: DAILY.title, body: "", dailyDate: DAILY.dailyDate, createdAt: "", updatedAt: "r2", trashed: false, trashedAt: null },
  section: "- Follow-through", start: 0, end: 10, taskIds: ["note:daily:2026-11-16:m"], checklistIds: [], replayed: false,
});

afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

function renderReview(overrides: Partial<Parameters<typeof MeetingFollowThroughDialog>[0]> = {}) {
  const handlers = { onClose: vi.fn(), onSaved: vi.fn(), onAddNote: vi.fn() };
  render(<MeetingFollowThroughDialog event={event} context={context()} today="2026-11-16" {...handlers} {...overrides} />);
  return { ...handlers, dialog: within(screen.getByRole("dialog", { name: "Review Follow-through" })) };
}

describe("MeetingFollowThroughDialog", () => {
  it("records typed and picked items, previews the exact section, and saves once to the chosen note", async () => {
    const save = vi.spyOn(mailClient, "saveMeetingFollowThrough").mockImplementation(async (_ref, _dest, _rev, operationId) => saved(operationId));
    const { dialog, onSaved } = renderReview();
    expect(dialog.getByRole("button", { name: "Add Decision" })).toHaveFocus();
    expect(dialog.getByLabelText("Source excerpt")).toHaveTextContent("Ship on Friday");
    expect(dialog.getByRole("button", { name: "Save Follow-through" })).toBeDisabled();
    expectPrimaryActionLast(dialog.getByRole("button", { name: "Save Follow-through" }).parentElement!);

    fireEvent.click(within(dialog.getByRole("list", { name: "Lines in the source" })).getAllByRole("button", { name: "Add as Decision" })[0]);
    fireEvent.click(dialog.getByRole("button", { name: "Add Action" }));
    expect(dialog.getByRole("textbox", { name: "Item 2" })).toHaveFocus();
    fireEvent.change(dialog.getByRole("textbox", { name: "Item 2" }), { target: { value: "Book the room" } });
    fireEvent.change(dialog.getByRole("combobox", { name: "Kind of Item 2" }), { target: { value: "checklist" } });
    fireEvent.click(dialog.getByRole("button", { name: "Link Task" }));
    expect(dialog.getByText("Adds after this meeting's line in Daily note, Mon, Nov 16.")).toBeInTheDocument();
    expect(dialog.getByLabelText("Insertion preview").textContent).toMatch(/^- Follow-through 2026-11-16: \[Sync\]\(threestrands:event\/.*\) <!-- threestrands-follow-through -->\n {2}- Decision: Ship on Friday\n {2}- \[ \] Book the room\n {2}- \[Review the contract\]\(threestrands:task\/.*\)$/);
    expect(save).not.toHaveBeenCalled();

    fireEvent.change(dialog.getByRole("combobox", { name: "Save To" }), { target: { value: "n-sync" } });
    expect(dialog.getByText("Adds at the end of “Sync notes”, linking this meeting.")).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Save Follow-through" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledOnce());
    expect(save).toHaveBeenCalledWith(reference, { noteId: "n-sync" }, "r9", expect.any(String), {
      date: "2026-11-16", meeting: "Sync",
      items: [{ kind: "decision", text: "Ship on Friday" }, { kind: "checklist", text: "Book the room" }, { kind: "link", title: "Review the contract", task: { version: 1, kind: "task", id: "task-9" } }],
    });
    expect(readFollowThroughRecovery(reference)).toBeNull();
  });

  it("keeps the review after a failed save and retries with the same operation, even after a restart", async () => {
    const save = vi.spyOn(mailClient, "saveMeetingFollowThrough").mockRejectedValueOnce(new Error("That note changed"));
    const first = renderReview();
    fireEvent.click(first.dialog.getByRole("button", { name: "Add Action" }));
    fireEvent.change(first.dialog.getByRole("textbox", { name: "Item 1" }), { target: { value: "Send the deck" } });
    fireEvent.click(first.dialog.getByRole("button", { name: "Save Follow-through" }));
    expect(await first.dialog.findByRole("alert")).toHaveTextContent("That note changed");
    expect(first.dialog.getByRole("textbox", { name: "Item 1" })).toHaveValue("Send the deck");
    const operation = save.mock.calls[0]![3];
    cleanup();

    // A restart restores the edits and the operation, without saving anything.
    save.mockImplementation(async (_ref, _dest, _rev, operationId) => saved(operationId));
    const second = renderReview();
    expect(second.dialog.getByRole("status")).toHaveTextContent("Restored the review you hadn't saved.");
    expect(second.dialog.getByRole("textbox", { name: "Item 1" })).toHaveValue("Send the deck");
    expect(save).toHaveBeenCalledTimes(1);
    fireEvent.click(second.dialog.getByRole("button", { name: "Save Follow-through" }));
    await waitFor(() => expect(second.onSaved).toHaveBeenCalledOnce());
    expect(save.mock.calls[1]![3]).toBe(operation);
  });

  it("discards a restored review on request, and keeps none for an empty one", () => {
    saveFollowThroughRecovery({ reference, reviewId: "r", operationId: "o", destinationNoteId: DAILY.noteId, items: [{ id: "i", kind: "task", text: "Old" }], updatedAt: 0 });
    const { dialog, onClose } = renderReview();
    expect(dialog.getByRole("textbox", { name: "Item 1" })).toHaveValue("Old");
    fireEvent.click(dialog.getByRole("button", { name: "Discard" }));
    expect(dialog.queryByRole("textbox", { name: "Item 1" })).not.toBeInTheDocument();
    expect(readFollowThroughRecovery(reference)).toBeNull();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("offers Add Note when the meeting has no note to save into", () => {
    const { dialog, onAddNote, onClose } = renderReview({ context: context({ thisMeeting: section([]) }) });
    expect(dialog.queryByRole("combobox", { name: "Save To" })).not.toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Add Note" }));
    expect(onClose).toHaveBeenCalled();
    expect(onAddNote).toHaveBeenCalled();
  });
});

describe("MeetingFollowUpDialog", () => {
  const items = [{ kind: "decision" as const, text: "Ship" }, { kind: "task" as const, text: "Send the deck" }];
  const open = (overrides: Partial<DraftHandoff> = {}): DraftHandoff => ({ handoffId: "h", draftId: "d", state: "open", draft: null, created: true, ...overrides });
  function renderFollowUp(onOpen = vi.fn(async (request: { handoffId: string }) => open({ handoffId: request.handoffId })), mailAccounts = [{ email: ME, label: "Work" }]) {
    const onClose = vi.fn();
    render(<MeetingFollowUpDialog event={event} context={context()} items={items} mailAccounts={mailAccounts} ownAddresses={[ME, "cal@example.com"]} onClose={onClose} onOpen={onOpen} />);
    return { onOpen, onClose };
  }
  beforeEach(() => { vi.spyOn(mailClient, "draftHandoffStatus").mockResolvedValue(null); });

  it("reviews account, external recipients, subject, and body, then opens one durable draft", async () => {
    const { onOpen, onClose } = renderFollowUp();
    const dialog = within(screen.getByRole("dialog", { name: "Draft Follow-up" }));
    expect(dialog.getByRole("combobox", { name: "Reply To" })).toHaveFocus();
    expect(dialog.getByRole("combobox", { name: "From" })).toHaveValue(ME);
    expect(dialog.getByText("ann@example.org")).toBeInTheDocument();
    expect(dialog.queryByText("cal@example.com")).not.toBeInTheDocument();
    expect(dialog.queryByRole("textbox", { name: /Bcc/ })).not.toBeInTheDocument();
    expect(dialog.getByRole("textbox", { name: "Message" })).toHaveValue("Thanks for joining Sync. A quick summary:\n\nDecisions:\n- Ship\n\nNext steps:\n- Send the deck\n");
    fireEvent.change(dialog.getByRole("textbox", { name: "Subject" }), { target: { value: "Notes from Sync" } });
    fireEvent.click(dialog.getByRole("button", { name: "Open Draft" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const request = onOpen.mock.calls[0]![0];
    expect(request).toMatchObject({ account: ME, mode: "new", sourceId: null, to: "ann@example.org", cc: "", subject: "Notes from Sync" });
    expect(readFollowUpHandoff(reference)).toBe(request.handoffId);
  });

  it("replies only in a conversation the user chooses, from its own account and message", async () => {
    vi.spyOn(mailClient, "getThread").mockResolvedValue({ thread: {} as never, messages: [{ id: "m1" }, { id: "m2" }] as never });
    const { onOpen } = renderFollowUp();
    const dialog = within(screen.getByRole("dialog", { name: "Draft Follow-up" }));
    fireEvent.change(dialog.getByRole("combobox", { name: "Reply To" }), { target: { value: "t1" } });
    expect(dialog.queryByRole("textbox", { name: "Subject" })).not.toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Open Draft" }));
    await waitFor(() => expect(onOpen).toHaveBeenCalled());
    expect(onOpen.mock.calls[0]![0]).toMatchObject({ mode: "replyAll", sourceId: "m2", account: ME });
  });

  it("resumes an earlier handoff instead of making another, and offers a new draft once it was sent", async () => {
    rememberFollowUpHandoff(reference, "h-old");
    const status = vi.spyOn(mailClient, "draftHandoffStatus").mockResolvedValue(open({ handoffId: "h-old" }));
    const { onOpen } = renderFollowUp();
    const dialog = within(await screen.findByRole("dialog", { name: "Draft Follow-up" }));
    expect(await dialog.findByText("A follow-up draft for this meeting is in Drafts.")).toBeInTheDocument();
    fireEvent.click(dialog.getByRole("button", { name: "Open Draft" }));
    await waitFor(() => expect(onOpen).toHaveBeenCalled());
    expect(onOpen.mock.calls[0]![0].handoffId).toBe("h-old");
    expect(status).toHaveBeenCalledWith("h-old");
    cleanup();

    status.mockResolvedValue(open({ handoffId: "h-old", state: "sent" }));
    renderFollowUp();
    const sent = within(await screen.findByRole("dialog", { name: "Draft Follow-up" }));
    expect(await sent.findByText("The follow-up for this meeting was sent.")).toBeInTheDocument();
    expect(sent.queryByRole("button", { name: "Open Draft" })).not.toBeInTheDocument();
    fireEvent.click(sent.getByRole("button", { name: "New Draft" }));
    expect(await sent.findByRole("textbox", { name: "Message" })).toBeInTheDocument();
    expect(readFollowUpHandoff(reference)).toBeNull();
  });

  it("keeps the review when opening the draft fails", async () => {
    const onOpen = vi.fn(async () => { throw new Error("Couldn't save the open draft"); });
    renderFollowUp(onOpen);
    const dialog = within(screen.getByRole("dialog", { name: "Draft Follow-up" }));
    fireEvent.change(dialog.getByRole("textbox", { name: "Message" }), { target: { value: "Edited" } });
    fireEvent.click(dialog.getByRole("button", { name: "Open Draft" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("Couldn't save the open draft");
    expect(dialog.getByRole("textbox", { name: "Message" })).toHaveValue("Edited");
  });

  it("lets a calendar-only setup copy the text but not open a draft", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { onOpen } = renderFollowUp(undefined, []);
    const dialog = within(screen.getByRole("dialog", { name: "Draft Follow-up" }));
    expect(dialog.queryByRole("button", { name: "Open Draft" })).not.toBeInTheDocument();
    await act(async () => { fireEvent.click(dialog.getByRole("button", { name: "Copy Text" })); });
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("Decisions:\n- Ship"));
    expect(onOpen).not.toHaveBeenCalled();
  });
});
