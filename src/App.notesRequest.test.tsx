import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import type { MeetingContext, ScheduleEvent, WorkspaceFile } from "./domain";
import { noteReferenceHref, type NoteReference } from "./noteEntities";
import { clearScheduleCache } from "./calendarScheduleCache";
import { todayTarget } from "./noteDrafts";
import { addDays, dateInputValue } from "./calendarTime";
import { lastEditorProps } from "./test/NoteEditorStub";

vi.mock("./NoteEditor", () => import("./test/NoteEditorStub"));

afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

it("consumes the Files note jump so the Notes sidebar restores the last note", async () => {
  localStorage.clear();
  const requested = await mailClient.createNote("Requested source note", "source body");
  const last = await mailClient.createNote("Last selected note", "last body");
  const file: WorkspaceFile = { accountId: "", messageId: "", threadId: "", subject: requested.title,
    sentAt: requested.updatedAt, sender: "", direction: "received", starred: false, availability: "offline",
    attachment: { id: "0123456789abcdef0123456789abcdef", filename: "source.bin", mimeType: "application/octet-stream", size: 8 },
    note: { id: requested.id, title: requested.title, dailyDate: null } };
  vi.spyOn(mailClient, "listFiles").mockResolvedValue({ files: [file], total: 1 });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Files (6)" }));
  const context = within(await screen.findByRole("complementary", { name: "File context" }));
  fireEvent.click(await context.findByRole("button", { name: requested.title }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(requested.title));
  fireEvent.click(await within(screen.getByRole("list", { name: "Notes" })).findByRole("button", { name: new RegExp(last.title) }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(last.title));
  fireEvent.click(screen.getByRole("button", { name: "Inbox (1)" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Note text" })).not.toBeInTheDocument());
  fireEvent.click(screen.getByRole("button", { name: "Notes (4)" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(last.title));
});

it("edits the selected day's note in Today through keyboard commands and opens that same note in Notes", async () => {
  localStorage.clear();
  const tomorrow = dateInputValue(addDays(new Date(), 1));
  await mailClient.saveDailyNote(tomorrow, "Tomorrow’s daily note", null);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  await screen.findByRole("region", { name: "Today" });
  fireEvent.keyDown(document.body, { key: "=" });
  fireEvent.keyDown(document.body, { key: "Tab" });
  const panel = screen.getByRole("region", { name: "Daily Note" });
  expect(screen.getByRole("heading", { name: "Daily Note" })).toBeInTheDocument();
  const editor = await screen.findByRole("textbox", { name: "Daily Note Text" });
  await waitFor(() => expect(editor).toHaveValue("Tomorrow’s daily note"));
  await waitFor(() => expect(editor).not.toHaveAttribute("readonly"));
  expect(editor).not.toHaveFocus();
  fireEvent.keyDown(panel, { key: "Enter" });
  expect(editor).toHaveFocus();
  fireEvent.change(editor, { target: { value: "Tomorrow’s edited plan" } });
  fireEvent.keyDown(editor, { key: "Escape" });
  expect(panel).toHaveFocus();
  fireEvent.click(screen.getByRole("button", { name: "Open in Notes" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue("Tomorrow’s edited plan"));
  expect((await mailClient.getDailyNote(tomorrow))?.body).toBe("Tomorrow’s edited plan");
  expect(tomorrow).not.toBe(todayTarget().date);
  fireEvent.click(screen.getByRole("button", { name: "Today (0)" }));
  await screen.findByRole("region", { name: "Today" });
  expect(screen.getByRole("heading", { name: "Daily Note" })).toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Daily Note Text" })).toHaveValue("Tomorrow’s edited plan"));
});

it("opens a note found from the command palette", async () => {
  localStorage.clear();
  const note = await mailClient.createNote("Palette quarterly plan", "zephyrine milestones");
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: /^Command Palette/ }));
  fireEvent.change(screen.getByRole("textbox", { name: "Filter Commands" }), { target: { value: "zephyrine" } });
  const notes = await screen.findByRole("region", { name: "Notes" });
  fireEvent.click(within(notes).getByRole("button", { name: new RegExp(note.title) }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
  expect(screen.queryByRole("textbox", { name: "Filter Commands" })).not.toBeInTheDocument();
});

it("opens a linked contact, lists its note, and returns with Escape preserving the note and search", async () => {
  localStorage.clear();
  const contact = await mailClient.saveContactProfile({ id: null, displayName: "Pete Linkfollow", role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: false, addresses: ["pete.linkfollow@example.com"], birthday: null });
  const href = noteReferenceHref({ version: 1, kind: "contact", id: "pete.linkfollow@example.com", contact: contact.id });
  const note = await mailClient.createNote("Meeting with Pete", `- met with [Pete](${href})`);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Notes (4)" }));
  fireEvent.click(await within(screen.getByRole("list", { name: "Notes" })).findByRole("button", { name: new RegExp(note.title) }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
  lastEditorProps.current!.onOpenEntity!(noteReferenceHref({ version: 1, kind: "contact", id: "nobody.saved@example.com" }));
  expect(await screen.findByText("No contact for nobody.saved@example.com on this device")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title);
  fireEvent.keyDown(document.body, { key: "/" });
  fireEvent.change(screen.getByRole("searchbox", { name: "Search Notes" }), { target: { value: "Meeting with Pete" } });
  const editedBody = `${note.body}\nFollow up on the agenda.`;
  fireEvent.change(screen.getByRole("textbox", { name: "Note text" }), { target: { value: editedBody } });
  lastEditorProps.current!.onOpenEntity!(href);
  expect(await screen.findByDisplayValue("Pete Linkfollow")).toBeInTheDocument();
  const notes = await within(await screen.findByRole("complementary", { name: "Contact context" })).findByRole("region", { name: "Notes" });
  expect(within(notes).getByRole("button", { name: /Meeting with Pete/ })).toBeInTheDocument();

  // Escape dismisses an overlay or the contact search before returning to Notes.
  fireEvent.click(screen.getByRole("button", { name: /^Command Palette/ }));
  await screen.findByRole("textbox", { name: "Filter Commands" });
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Filter Commands" })).not.toBeInTheDocument());
  expect(screen.getByDisplayValue("Pete Linkfollow")).toBeInTheDocument();
  const search = screen.getByRole("textbox", { name: "Search Contacts" });
  fireEvent.focus(search);
  fireEvent.keyDown(search, { key: "Escape" });
  expect(screen.getByDisplayValue("Pete Linkfollow")).toBeInTheDocument();

  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
  expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue(editedBody);
  expect(screen.getByRole("searchbox", { name: "Search Notes" })).toHaveValue("Meeting with Pete");
});

it("opens a meeting from Today in its daily note, and Undo takes the new entry out", async () => {
  localStorage.clear();
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting = { id: "app-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Wiring review", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString(), attendees: ["dana.wiring@example.com"] };
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" }]);
  vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting], errors: [] });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  fireEvent.click(await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: "Add Note: Wiring review" }));
  await screen.findByRole("textbox", { name: "Note text" });
  expect(await screen.findByText(/Added to (today|tomorrow)'s note/)).toBeInTheDocument();
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add Note" })).not.toBeInTheDocument());
  const day = `${start.getFullYear()}-${String(start.getMonth() + 1).padStart(2, "0")}-${String(start.getDate()).padStart(2, "0")}`;
  const body = (await mailClient.getDailyNote(day))?.body ?? "";
  expect(body.startsWith("## Meetings\n\n- [")).toBe(true);
  expect(body).toContain("](threestrands:event/");
  expect(body).toContain(" met with [dana.wiring@example.com](threestrands:contact/");
  expect(body.endsWith(" for Wiring review\n")).toBe(true);
  await act(async () => { screen.getByRole("button", { name: "Undo" }).click(); });
  await waitFor(async () => expect((await mailClient.getDailyNote(day))?.body ?? "").toBe(""));
});

it("opens an existing occurrence note from Today instead of showing the add dialog", async () => {
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting = { id: "already-noted-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Existing review", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  const href = noteReferenceHref({ version: 1, kind: "event", id: meeting.id, account: meeting.accountId, calendar: meeting.calendarId });
  const note = await mailClient.createNote("Existing review notes", `[Meeting](${href})`);
  const add = vi.spyOn(mailClient, "addMeetingToDailyNote");
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" }]);
  vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting], errors: [] });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  fireEvent.click(await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: "Open Note: Existing review" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
  expect(screen.queryByRole("dialog", { name: "Add Note" })).not.toBeInTheDocument();
  expect(add).not.toHaveBeenCalled();
});

it.each(["Today", "Calendar"])("opens the backlinked meeting note from the %s agenda", async (workspace) => {
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting = { id: `backlinked-${workspace}`, accountId: "calendar@example.com", calendarId: "primary", title: `Forum from ${workspace}`, allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  const day = `${start.getFullYear()}-${String(start.getMonth() + 1).padStart(2, "0")}-${String(start.getDate()).padStart(2, "0")}`;
  const daily = await mailClient.getDailyNote(day);
  const capture = await mailClient.addMeetingToDailyNote(day, daily?.updatedAt ?? null, {
    event: { version: 1, kind: "event", id: meeting.id, account: meeting.accountId, calendar: meeting.calendarId },
    time: "10:00 am", attendees: [], name: meeting.title, linkNote: true,
  });
  mockMeetingCalendar(meeting);
  const add = vi.spyOn(mailClient, "addMeetingToDailyNote");
  try {
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: workspace === "Today" ? "Today (0)" : "Calendar (2)" }));
    const agenda = workspace === "Today" ? await screen.findByRole("group", { name: "Day view" }) : await screen.findByRole("region", { name: "Today's agenda" });
    fireEvent.click(await within(agenda).findByRole("button", { name: workspace === "Today" ? `Open Note: ${meeting.title}` : `Add Note for ${meeting.title}` }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(capture.meetingNote!.title));
    expect(add).not.toHaveBeenCalled();
  } finally {
    cleanup();
    const current = await mailClient.getDailyNote(day);
    await mailClient.saveDailyNote(day, daily?.body ?? "", current!.updatedAt);
    await mailClient.trashNote(capture.meetingNote!.id);
  }
});

it("adds a backlinked meeting from the Notes agenda and opens its dedicated note from the confirmation", async () => {
  localStorage.clear();
  clearScheduleCache();
  const now = new Date();
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 12);
  const meeting = { id: "notes-rail-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Rail sync", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" }]);
  vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting], errors: [] });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Notes (4)" }));
  const agenda = within(await screen.findByRole("region", { name: "Agenda" }));
  fireEvent.click(await agenda.findByRole("button", { name: "Add Note for Rail sync" }));
  const dialog = within(await screen.findByRole("dialog", { name: "Add Note" }));
  fireEvent.click(dialog.getByRole("checkbox", { name: "Create backlinked note" }));
  fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
  expect(await screen.findByText("Added to today's note")).toBeInTheDocument();
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Note text" }) as HTMLTextAreaElement).value).toMatch(/^## Meetings\n\n- \[.+\) for \[\[Rail sync\]\]\n$/));
  fireEvent.click(screen.getByRole("button", { name: "Open Note" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Rail sync"));
});

it("hides and shows the Notes side panel with a backslash", async () => {
  localStorage.clear();
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Notes (4)" }));
  expect(await screen.findByRole("complementary", { name: "Note context" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "\\" });
  await waitFor(() => expect(screen.queryByRole("complementary", { name: "Note context" })).not.toBeInTheDocument());
  expect(screen.getByRole("button", { name: "Show Note context" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "\\" });
  expect(await screen.findByRole("complementary", { name: "Note context" })).toBeInTheDocument();
});

function prepContext(reference: NoteReference, noteId: string): MeetingContext {
  const section = <T,>(items: T[]) => ({ status: "ready" as const, items, omitted: 0, message: null });
  return {
    reference, event: null, mailScope: [],
    thisMeeting: section([{ noteId, title: "Prep source", dailyDate: null, revision: "r", kind: "excerpt" as const, text: "- Agenda for the review", truncated: false, start: 0, end: 10 }]),
    previousMeetings: section([]), seriesNotes: section([]), conversations: section([]), commitments: section([]), followThrough: section([]),
    limits: { noteSources: 20, priorOccurrences: 8, excerptBytes: 4096, linkedNotePreviewBytes: 1024, attendees: 50, conversations: 10, mailLookbackDays: 90, commitments: 25 },
  };
}

function mockMeetingCalendar(meeting: ScheduleEvent) {
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" }]);
  vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting], errors: [] });
}

it("prepares a meeting beside its note, opens a source note, and returns to the same preparation", async () => {
  localStorage.clear();
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting: ScheduleEvent = { id: "prep-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Prep review", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  const source = await mailClient.createNote("Prep source", "- Agenda for the review");
  const reference = { version: 1 as const, kind: "event" as const, id: meeting.id, account: meeting.accountId, calendar: meeting.calendarId! };
  const meetingNote = await mailClient.createNote("Prep meeting notes", `[Prep review](${noteReferenceHref(reference)})`);
  vi.spyOn(mailClient, "resolveNoteEvent").mockResolvedValue(meeting);
  vi.spyOn(mailClient, "meetingContext").mockImplementation(async (reference) => prepContext(reference, source.id));
  mockMeetingCalendar(meeting);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  fireEvent.click(await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: "Open Note: Prep review" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(meetingNote.title));
  fireEvent.click(await screen.findByRole("button", { name: "Prepare Meeting: Prep review" }));
  const prep = within(await screen.findByRole("region", { name: "Preparation for Prep review" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Note text" }), { target: { value: `${meetingNote.body}\nNotes written during preparation` } });
  fireEvent.click(await prep.findByRole("button", { name: /Agenda for the review/ }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Prep source"));
  expect((await mailClient.getNote(meetingNote.id))?.body).toContain("Notes written during preparation");
  fireEvent.keyDown(window, { key: "Escape" });
  fireEvent.click(await screen.findByRole("button", { name: "Back to Meeting" }));
  expect(await screen.findByRole("region", { name: "Preparation for Prep review" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Close Preparation" }));
  await waitFor(() => expect(screen.queryByRole("region", { name: "Preparation for Prep review" })).not.toBeInTheDocument());
  expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(meetingNote.title);
  // Today’s p shortcut opens the note; preparation stays beside the editor.
  fireEvent.click(screen.getByRole("button", { name: "Today (0)" }));
  await screen.findByRole("group", { name: "Day view" });
  fireEvent.keyDown(window, { key: "p" });
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(meetingNote.title));
  fireEvent.click(await screen.findByRole("button", { name: "Prepare Meeting: Prep review" }));
  expect(await screen.findByRole("region", { name: "Preparation for Prep review" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("region", { name: "Preparation for Prep review" })).not.toBeInTheDocument());
  expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(meetingNote.title);
});

it("reviews follow-through from preparation with Undo, and hands a follow-up to the composer", async () => {
  localStorage.clear();
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting: ScheduleEvent = { id: "follow-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Follow review", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString(), attendees: ["ann@example.org"] };
  const reference = { version: 1 as const, kind: "event" as const, id: meeting.id, account: meeting.accountId, calendar: meeting.calendarId! };
  const source = await mailClient.createNote("Follow-through source", `[Follow review](${noteReferenceHref(reference)})`);
  vi.spyOn(mailClient, "resolveNoteEvent").mockResolvedValue(meeting);
  vi.spyOn(mailClient, "meetingContext").mockImplementation(async (reference) => ({ ...prepContext(reference, source.id),
    event: { title: meeting.title, start: meeting.start, end: meeting.end, allDay: false, seriesId: null, attendees: ["ann@example.org"], attendeesOmitted: 0, sourceMessageId: null, responseStatus: null } }));
  const save = vi.spyOn(mailClient, "saveMeetingFollowThrough").mockImplementation(async (_reference, _destination, _revision, operationId) => ({
    operationId, note: source, section: "- Follow-through", start: 0, end: 1, taskIds: [], checklistIds: [], replayed: false,
  }));
  const undo = vi.spyOn(mailClient, "undoMeetingFollowThrough").mockResolvedValue({ note: source });
  vi.spyOn(mailClient, "draftHandoffStatus").mockResolvedValue(null);
  const handoff = vi.spyOn(mailClient, "draftHandoff").mockImplementation(async (request) => ({
    handoffId: request.handoffId, draftId: "follow-up", state: "open", created: true,
    draft: { id: "follow-up", revision: 1, account: request.account, mode: "new", sourceId: null, threadId: null, replyId: null, references: [], to: request.to, cc: "", bcc: "", subject: request.subject, body: request.body, attachments: [], updatedAt: 1 },
  }));
  mockMeetingCalendar(meeting);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  fireEvent.click(await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: "Open Note: Follow review" }));
  fireEvent.click(await screen.findByRole("button", { name: "Prepare Meeting: Follow review" }));
  const prep = within(await screen.findByRole("region", { name: "Preparation for Follow review" }));
  fireEvent.click(await prep.findByRole("button", { name: "Review Follow-through" }));
  const review = within(await screen.findByRole("dialog", { name: "Review Follow-through" }));
  fireEvent.click(review.getByRole("button", { name: "Add Action" }));
  fireEvent.change(review.getByRole("textbox", { name: "Item 1" }), { target: { value: "Send the notes" } });
  fireEvent.click(review.getByRole("button", { name: "Save Follow-through" }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(await screen.findByText("Follow-through saved to Follow-through source")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Undo" }));
  await waitFor(() => expect(undo).toHaveBeenCalledWith(save.mock.calls[0]![3]));

  fireEvent.click(prep.getByRole("button", { name: "Draft Follow-up" }));
  const draft = within(await screen.findByRole("dialog", { name: "Draft Follow-up" }));
  // The calendar's account sends no mail, so the account is an explicit choice.
  const from = draft.getByRole("combobox", { name: "From" });
  expect(from).toHaveValue("");
  fireEvent.change(from, { target: { value: within(from).getAllByRole("option")[1]!.getAttribute("value") } });
  fireEvent.change(draft.getByRole("textbox", { name: "Subject" }), { target: { value: "Notes from the review" } });
  fireEvent.click(await draft.findByRole("button", { name: "Open Draft" }));
  await waitFor(() => expect(handoff).toHaveBeenCalledOnce());
  expect(await screen.findByRole("textbox", { name: "Subject" })).toHaveValue("Notes from the review");
});

it("prepares the meeting open in Calendar from its details and from the command palette", async () => {
  localStorage.clear();
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting: ScheduleEvent = { id: "calendar-prep", accountId: "calendar@example.com", calendarId: "primary", title: "Calendar prep", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  const source = await mailClient.createNote("Calendar prep source", "- Agenda for the review");
  vi.spyOn(mailClient, "meetingContext").mockImplementation(async (reference) => prepContext(reference, source.id));
  mockMeetingCalendar(meeting);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  fireEvent.click(await within(await screen.findByRole("group", { name: "Day view" })).findByRole("button", { name: /^Calendar prep/ }));
  const rail = within(await screen.findByRole("complementary", { name: "Meeting context" }));
  fireEvent.click(await rail.findByRole("button", { name: "Prepare" }));
  expect(await rail.findByRole("region", { name: "Preparation for Calendar prep" })).toBeInTheDocument();
  fireEvent.click(rail.getByRole("button", { name: "Close Preparation" }));
  await waitFor(() => expect(rail.queryByRole("region", { name: "Preparation for Calendar prep" })).not.toBeInTheDocument());

  fireEvent.click(screen.getByRole("button", { name: /^Command Palette/ }));
  fireEvent.change(screen.getByRole("textbox", { name: "Filter Commands" }), { target: { value: "Prepare for Meeting" } });
  fireEvent.click(await screen.findByRole("button", { name: /Prepare for Meeting/ }));
  expect(await rail.findByRole("region", { name: "Preparation for Calendar prep" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(rail.queryByRole("region", { name: "Preparation for Calendar prep" })).not.toBeInTheDocument());
  fireEvent.keyDown(window, { key: "p" });
  expect(await rail.findByRole("region", { name: "Preparation for Calendar prep" })).toBeInTheDocument();
});
