import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import type { MeetingContext, ScheduleEvent, WorkspaceFile } from "./domain";
import { noteReferenceHref, type NoteReference } from "./noteEntities";
import { clearScheduleCache } from "./calendarScheduleCache";
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

it("adds a meeting from Today's next-meeting card to today's note, and Undo takes it out", async () => {
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
  await screen.findByRole("button", { name: /^Next: Wiring review/ });
  fireEvent.click(screen.getByRole("button", { name: "Add Note" }));
  const dialog = within(await screen.findByRole("dialog", { name: "Add Note" }));
  expect(dialog.getByText("dana.wiring@example.com")).toBeInTheDocument();
  fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
  expect(await screen.findByRole("status")).toHaveTextContent(/Added to (today|tomorrow)'s note/);
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
  await screen.findByRole("button", { name: /^Next: Existing review/ });
  fireEvent.click(screen.getByRole("button", { name: "Add Note" }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
  expect(screen.queryByRole("dialog", { name: "Add Note" })).not.toBeInTheDocument();
  expect(add).not.toHaveBeenCalled();
});

it("adds a meeting from a daily note's agenda in Notes, and the open note shows it", async () => {
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
  fireEvent.click(dialog.getByRole("button", { name: "Add to Daily Note" }));
  expect(await screen.findByText("Added to today's note")).toBeInTheDocument();
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Note text" }) as HTMLTextAreaElement).value).toMatch(/^## Meetings\n\n- \[.+\) for Rail sync\n$/));
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
    previousMeetings: section([]), seriesNotes: section([]), conversations: section([]), commitments: section([]),
    limits: { noteSources: 20, priorOccurrences: 8, excerptBytes: 4096, linkedNotePreviewBytes: 1024, attendees: 50, conversations: 10, mailLookbackDays: 90, commitments: 25 },
  };
}

function mockMeetingCalendar(meeting: ScheduleEvent) {
  vi.spyOn(mailClient, "listCalendarAccounts").mockResolvedValue([{ email: "calendar@example.com", connectedAt: "2026-09-18T00:00:00Z", status: "connected" }]);
  vi.spyOn(mailClient, "listCalendarOptions").mockResolvedValue([{ id: "primary", accountId: "calendar@example.com", name: "Work", primary: true, selected: true, writable: true }]);
  vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting], errors: [] });
}

it("prepares a meeting on Today, opens a source note, and comes back to the same preparation", async () => {
  localStorage.clear();
  clearScheduleCache();
  const start = new Date(Date.now() + 10 * 60_000);
  const meeting: ScheduleEvent = { id: "prep-meeting", accountId: "calendar@example.com", calendarId: "primary", title: "Prep review", allDay: false,
    start: start.toISOString(), end: new Date(start.getTime() + 30 * 60_000).toISOString() };
  const source = await mailClient.createNote("Prep source", "- Agenda for the review");
  vi.spyOn(mailClient, "meetingContext").mockImplementation(async (reference) => prepContext(reference, source.id));
  mockMeetingCalendar(meeting);
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Today (0)" }));
  await screen.findByRole("button", { name: /^Next: Prep review/ });
  fireEvent.click(screen.getByRole("button", { name: "Prepare" }));
  const prep = within(await screen.findByRole("region", { name: "Preparation for Prep review" }));
  fireEvent.click(await prep.findByRole("button", { name: /Agenda for the review/ }));
  await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Prep source"));
  fireEvent.click(await screen.findByRole("button", { name: "Back to Meeting" }));
  expect(await screen.findByRole("region", { name: "Preparation for Prep review" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Close Preparation" }));
  await waitFor(() => expect(screen.queryByRole("region", { name: "Preparation for Prep review" })).not.toBeInTheDocument());
  // Closing preparation brings back today's plan in the left column.
  expect(screen.getByRole("group", { name: "Plan" })).toBeInTheDocument();

  // p prepares the next meeting and Escape closes it.
  fireEvent.keyDown(window, { key: "p" });
  expect(await screen.findByRole("region", { name: "Preparation for Prep review" })).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "Today's agenda" })).toBeInTheDocument();
  fireEvent.keyDown(window, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("region", { name: "Preparation for Prep review" })).not.toBeInTheDocument());
  // Escape closed preparation without leaving Today.
  expect(screen.getByRole("region", { name: "Today's agenda" })).toBeInTheDocument();
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
  fireEvent.click(await screen.findByRole("button", { name: /^Next: Calendar prep/ }));
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
