import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AddToNoteDialog } from "./AddToNoteDialog";
import { clearScheduleCache } from "./calendarScheduleCache";
import { ADD_TO_NOTE_MEETING_DAYS, MAX_ADDITION_FILES, MAX_ADDITION_QUOTE_CHARS, MAX_MEETING_NOTE_SUGGESTIONS, MAX_NOTE_ATTACHMENT_BYTES, MAX_NOTE_SEARCH_SUGGESTIONS, MAX_RECENT_NOTE_SUGGESTIONS } from "./addToNotePolicy";
import { mailClient } from "./data/client";
import type { Account, Message, MessageAttachment, NoteSummary, ScheduleEvent, Thread } from "./domain";
import { expectPrimaryActionLast } from "./test/sharedButtons";

const now = new Date(2026, 10, 12, 8, 0);
const thread = { id: "thread-1", providerThreadId: "p1", subject: "Q3 plan", accountId: "you@example.com" } as Thread;
const accounts = [{ email: "you@example.com", displayName: "You" }] as Account[];
const file = (id: string, filename: string, mimeType: string, size = 100, extra: Partial<MessageAttachment> = {}): MessageAttachment => ({ id, filename, mimeType, size, ...extra });
const message = (extra: Partial<Message> = {}): Message => ({
  id: "m1", threadId: "thread-1", sender: "Ada Lovelace <ada@example.com>", recipients: ["you@example.com"], sentAt: new Date(2026, 9, 30, 9, 0).toISOString(),
  bodyHtml: "<p>Hello</p>", bodyText: "Hello Ada\nsecond line", unread: false, attachments: [], ...extra,
});
const note = (id: string, title: string, dailyDate: string | null = null): NoteSummary => ({ id, title, dailyDate, preview: "", updatedAt: "u", visitedAt: null, trashedAt: null });
const meeting = (id: string, title: string, hours: number, attendees = ["ada@example.com"]): ScheduleEvent => {
  const start = new Date(now.getTime() + hours * 3_600_000);
  return { id, accountId: "you@example.com", calendarId: "work", title, allDay: false, start: start.toISOString(), end: new Date(start.getTime() + 1_800_000).toISOString(), attendees };
};

function renderDialog(props: Partial<Parameters<typeof AddToNoteDialog>[0]> = {}) {
  const onClose = vi.fn(), onSubmit = vi.fn(async () => undefined);
  const view = render(<AddToNoteDialog thread={thread} message={message()} quote={null} accounts={accounts} ownAddresses={["you@example.com"]}
    people={[{ email: "ada@example.com", name: "Ada Lovelace" }]} calendarConnected={false} timeZone="UTC" now={now} onClose={onClose} onSubmit={onSubmit} {...props} />);
  return { ...view, onClose, onSubmit, dialog: within(screen.getByRole("dialog", { name: "Add to Note" })) };
}
const optionLabels = (dialog: { getAllByRole(role: string): HTMLElement[] }) => dialog.getAllByRole("option").map((option) => option.querySelector("span")?.textContent);

describe("AddToNoteDialog", () => {
  beforeEach(() => {
    clearScheduleCache();
    vi.spyOn(mailClient, "listRecentNotes").mockResolvedValue([note("n1", "Roadmap"), note("daily:2026-11-12", "2026-11-12", "2026-11-12"), note("n2", "Hiring")]);
    vi.spyOn(mailClient, "searchNotes").mockResolvedValue([]);
    vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([]);
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [], errors: [] });
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("focuses the note picker first, with Cancel before the primary action", async () => {
    const { dialog } = renderDialog();
    expect(dialog.getByRole("combobox", { name: "Note" })).toHaveFocus();
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeEnabled();
    expectPrimaryActionLast(dialog.getByRole("button", { name: "Add to Note" }).parentElement!);
    await dialog.findByRole("option", { name: /Roadmap/ });
  });

  it("closes with Escape, restores focus, and keeps Tab inside", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const { dialog, onClose, unmount } = renderDialog();
    await dialog.findByRole("option", { name: /Roadmap/ });
    dialog.getByRole("button", { name: "Add to Note" }).focus();
    fireEvent.keyDown(dialog.getByRole("button", { name: "Add to Note" }), { key: "Tab" });
    expect(dialog.getByRole("button", { name: "Close" })).toHaveFocus();
    fireEvent.keyDown(dialog.getByRole("button", { name: "Close" }), { key: "Tab", shiftKey: true });
    expect(dialog.getByRole("button", { name: "Add to Note" })).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it("offers meetings with a participant first, then today's note, then recent notes", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [
      meeting("far", "Too far", 24 * 8), meeting("other", "No Ada", 3, ["bob@example.com"]),
      meeting("later", "Planning", 26), meeting("soon", "Standup", 2, ["ada@example.com", "you@example.com"]),
    ], errors: [] });
    vi.spyOn(mailClient, "noteBacklinks").mockImplementation(async (reference) => reference.id === "soon" ? [{ noteId: "m-note", title: "Standup notes", dailyDate: null, context: "" }, { noteId: "daily:2026-11-12", title: "2026-11-12", dailyDate: "2026-11-12", context: "" }] : []);
    const { dialog } = renderDialog({ calendarConnected: true });
    await waitFor(() => expect(optionLabels(dialog)).toEqual(["Standup notes", "Create a note for “Planning”", "Today's note", "Roadmap", "Hiring"]));
    const options = dialog.getAllByRole("option");
    expect(options[0]).toHaveTextContent(/meeting today 10:00/i);
    expect(options[1]).toHaveTextContent(/meeting tomorrow 10:00/i);
  });

  it("looks ahead only the meeting window: a meeting just inside shows, one at its end does not", async () => {
    const windowEnd = new Date(2026, 10, 12 + ADD_TO_NOTE_MEETING_DAYS).getTime();
    const at = (id: string, start: number): ScheduleEvent => ({ ...meeting(id, id, 0), start: new Date(start).toISOString(), end: new Date(start + 1_800_000).toISOString() });
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [at("inside", windowEnd - 60_000), at("boundary", windowEnd), at("beyond", windowEnd + 3_600_000)], errors: [] });
    const { dialog } = renderDialog({ calendarConnected: true });
    await waitFor(() => expect(optionLabels(dialog)).toEqual(["Create a note for “inside”", "Today's note", "Roadmap", "Hiring"]));
  });

  it("offers at most the limit of meetings and of recent notes, and asks search for at most its limit", async () => {
    const meetings = Array.from({ length: MAX_MEETING_NOTE_SUGGESTIONS + 1 }, (_, index) => meeting(`m${index}`, `Meeting ${index}`, 2 + index));
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: meetings, errors: [] });
    const recent = Array.from({ length: MAX_RECENT_NOTE_SUGGESTIONS + 3 }, (_, index) => note(`r${index}`, `Recent ${index}`));
    vi.spyOn(mailClient, "listRecentNotes").mockResolvedValue(recent);
    const search = vi.spyOn(mailClient, "searchNotes");
    const { dialog } = renderDialog({ calendarConnected: true });
    await waitFor(() => expect(optionLabels(dialog)).toHaveLength(MAX_MEETING_NOTE_SUGGESTIONS + 1 + MAX_RECENT_NOTE_SUGGESTIONS));
    expect(optionLabels(dialog).filter((label) => label?.startsWith("Create a note"))).toHaveLength(MAX_MEETING_NOTE_SUGGESTIONS);
    fireEvent.change(dialog.getByRole("combobox", { name: "Note" }), { target: { value: "r" } });
    await waitFor(() => expect(search).toHaveBeenCalledWith("r", MAX_NOTE_SEARCH_SUGGESTIONS));
  });

  it("searches notes while typing and offers to create one last", async () => {
    const search = vi.spyOn(mailClient, "searchNotes").mockResolvedValue([{ id: "n9", title: "Board prep", dailyDate: null, matchSnippet: "", trashed: false }]);
    const { dialog, onSubmit } = renderDialog();
    fireEvent.change(dialog.getByRole("combobox", { name: "Note" }), { target: { value: "board" } });
    await waitFor(() => expect(optionLabels(dialog)).toEqual(["Board prep", "Create note “board”"]));
    expect(search).toHaveBeenCalledWith("board", expect.any(Number));
    fireEvent.change(dialog.getByRole("combobox", { name: "Note" }), { target: { value: "a|b" } });
    await waitFor(() => expect(dialog.getByRole("option", { name: /Create note/ })).toHaveAttribute("aria-disabled", "true"));
    fireEvent.click(dialog.getByRole("option", { name: /Create note/ }));
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeDisabled();
    fireEvent.change(dialog.getByRole("combobox", { name: "Note" }), { target: { value: "board" } });
    fireEvent.click(await dialog.findByRole("option", { name: /Create note “board”/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ choice: { kind: "new", title: "board" } })));
  });

  it("writes nothing until submitted, and Cmd+Enter submits to the preselected note", async () => {
    const write = vi.spyOn(mailClient, "addToNote");
    const create = vi.spyOn(mailClient, "createNote");
    const { dialog, onSubmit } = renderDialog();
    await dialog.findByRole("option", { name: /Roadmap/ });
    expect(dialog.getByRole("option", { name: /Today's note/ })).toHaveAttribute("aria-selected", "true");
    expect(onSubmit).not.toHaveBeenCalled();
    fireEvent.keyDown(dialog.getByRole("combobox", { name: "Note" }), { key: "Enter", metaKey: true });
    await waitFor(() => expect(onSubmit).toHaveBeenCalledExactlyOnceWith({ choice: { kind: "note", id: "daily:2026-11-12" }, quote: null, dateLabel: "Oct 30", files: [] }));
    expect(write).not.toHaveBeenCalled();
    expect(create).not.toHaveBeenCalled();
  });

  it("preselects the first meeting's note, and Cmd+Enter adds there", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting("soon", "Standup", 2)], errors: [] });
    vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([{ noteId: "m-note", title: "Standup notes", dailyDate: null, context: "" }]);
    const { dialog, onSubmit } = renderDialog({ calendarConnected: true });
    await waitFor(() => expect(dialog.getByRole("option", { name: /Standup notes/ })).toHaveAttribute("aria-selected", "true"));
    expect(dialog.getByRole("option", { name: /Today's note/ })).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(dialog.getByRole("combobox", { name: "Note" }), { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ choice: { kind: "note", id: "m-note" } })));
  });

  it("moves the preselection to a meeting that arrives later, but never over the user's pick", async () => {
    let release: (value: { events: ScheduleEvent[]; errors: string[] }) => void = () => undefined;
    vi.spyOn(mailClient, "listScheduleEvents").mockReturnValue(new Promise((resolve) => { release = resolve; }));
    vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([{ noteId: "m-note", title: "Standup notes", dailyDate: null, context: "" }]);
    const { dialog } = renderDialog({ calendarConnected: true });
    await dialog.findByRole("option", { name: /Roadmap/ });
    expect(dialog.getByRole("option", { name: /Today's note/ })).toHaveAttribute("aria-selected", "true");
    release({ events: [meeting("soon", "Standup", 2)], errors: [] });
    await waitFor(() => expect(dialog.getByRole("option", { name: /Standup notes/ })).toHaveAttribute("aria-selected", "true"));
  });

  it("keeps the user's pick as suggestions finish loading", async () => {
    let release: (value: { events: ScheduleEvent[]; errors: string[] }) => void = () => undefined;
    vi.spyOn(mailClient, "listScheduleEvents").mockReturnValue(new Promise((resolve) => { release = resolve; }));
    vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([{ noteId: "m-note", title: "Standup notes", dailyDate: null, context: "" }]);
    const { dialog, onSubmit } = renderDialog({ calendarConnected: true });
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    release({ events: [meeting("soon", "Standup", 2)], errors: [] });
    await dialog.findByRole("option", { name: /Standup notes/ });
    expect(dialog.getByRole("option", { name: /Roadmap/ })).toHaveAttribute("aria-selected", "true");
    expect(dialog.getByRole("option", { name: /Standup notes/ })).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(dialog.getByRole("combobox", { name: "Note" }), { key: "Enter", metaKey: true });
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ choice: { kind: "note", id: "n1" } })));
  });

  it("chooses nothing from search results on its own", async () => {
    vi.spyOn(mailClient, "searchNotes").mockResolvedValue([{ id: "n9", title: "Board prep", dailyDate: null, matchSnippet: "", trashed: false }]);
    const { dialog } = renderDialog();
    fireEvent.change(dialog.getByRole("combobox", { name: "Note" }), { target: { value: "board" } });
    await dialog.findByRole("option", { name: /Board prep/ });
    expect(dialog.getAllByRole("option").every((option) => option.getAttribute("aria-selected") === "false")).toBe(true);
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeDisabled();
  });

  it("previews a selection as a quote with its source, and sends exactly that", async () => {
    const { dialog, onSubmit } = renderDialog({ quote: "the selected passage" });
    const adding = dialog.getByRole("region", { name: "Adding" });
    expect(within(adding).getByText("the selected passage")).toBeInTheDocument();
    expect(within(adding).getByText("From Ada Lovelace, Oct 30")).toBeInTheDocument();
    expect(within(adding).queryByRole("checkbox", { name: "Include message text" })).not.toBeInTheDocument();
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ quote: "the selected passage" })));
  });

  it("names your own mail by its recipient", () => {
    const { dialog } = renderDialog({ quote: "mine", message: message({ sender: "You <you@example.com>", recipients: ["Grace Hopper <grace@example.com>"] }) });
    expect(dialog.getByText("From your email to Grace Hopper, Oct 30")).toBeInTheDocument();
  });

  it("without a selection adds a link, and quotes the whole message only when asked", async () => {
    const { dialog, onSubmit } = renderDialog();
    const adding = dialog.getByRole("region", { name: "Adding" });
    expect(within(adding).getByText(/A link to the conversation, “Q3 plan”/)).toBeInTheDocument();
    const include = within(adding).getByRole("checkbox", { name: "Include message text" });
    expect(include).not.toBeChecked();
    fireEvent.click(include);
    expect(adding.querySelector("blockquote")).toHaveTextContent("Hello Ada second line");
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ quote: "Hello Ada\nsecond line" })));
  });

  it("refuses a passage over the limit and accepts one at the limit", async () => {
    for (const [length, refused] of [[MAX_ADDITION_QUOTE_CHARS - 1, false], [MAX_ADDITION_QUOTE_CHARS, false], [MAX_ADDITION_QUOTE_CHARS + 1, true]] as const) {
      const { dialog, unmount } = renderDialog({ quote: "x".repeat(length) });
      fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
      expect(dialog.queryByRole("alert") !== null).toBe(refused);
      expect((dialog.getByRole("button", { name: "Add to Note" }) as HTMLButtonElement).disabled).toBe(refused);
      unmount();
    }
  });

  it("lists each file unchecked, disables one over the note limit with its reason, and sends only the checked ones", async () => {
    const attachments = [
      file("a1", "shot.png", "image/png", 2048, { contentId: "cid1", inline: true }),
      file("a2", "report.pdf", "application/pdf", 4096),
      file("a3", "huge.zip", "application/zip", MAX_NOTE_ATTACHMENT_BYTES + 1),
      file("a4", "edge.bin", "application/octet-stream", MAX_NOTE_ATTACHMENT_BYTES),
    ];
    const { dialog, onSubmit } = renderDialog({ message: message({ attachments }) });
    const files = dialog.getByRole("group", { name: "Files" });
    expect(within(files).getByText("1 image and 3 other files from this message")).toBeInTheDocument();
    const boxes = within(files).getAllByRole("checkbox");
    expect(boxes.map((box) => (box as HTMLInputElement).checked)).toEqual([false, false, false, false]);
    expect(within(files).getByRole("checkbox", { name: /huge.zip/ })).toBeDisabled();
    expect(within(files).getByRole("checkbox", { name: /huge.zip/ })).toHaveAccessibleDescription(/20 MB/);
    expect(within(files).getByRole("checkbox", { name: /edge.bin/ })).toBeEnabled();
    fireEvent.click(within(files).getByRole("checkbox", { name: /shot.png/ }));
    fireEvent.click(within(files).getByRole("checkbox", { name: /edge.bin/ }));
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ files: ["a1", "a4"] })));
  });

  it("never offers an image the message loads from the web, and says how many were left out", () => {
    const html = '<p>Hi</p><img src="cid:logo"><img src="https://tracker.example/pixel.gif"><img src="http://cdn.example/hero.png">';
    const { dialog } = renderDialog({ message: message({ bodyHtml: html, attachments: [file("a1", "logo.png", "image/png", 10, { contentId: "logo", inline: true })] }) });
    const files = dialog.getByRole("group", { name: "Files" });
    expect(within(files).getAllByRole("checkbox")).toHaveLength(1);
    expect(files).toHaveTextContent(/images? loads? from the web and can't be copied/);
    expect(files).not.toHaveTextContent("pixel.gif");
    expect(files).toHaveTextContent("2 images load from the web");
  });

  it("offers no files section for a message without files or web images", () => {
    const { dialog } = renderDialog();
    expect(dialog.queryByRole("group", { name: "Files" })).not.toBeInTheDocument();
  });

  it("refuses more files than a note takes at once", async () => {
    const attachments = Array.from({ length: MAX_ADDITION_FILES + 1 }, (_, index) => file(`f${index}`, `f${index}.txt`, "text/plain"));
    const { dialog } = renderDialog({ message: message({ attachments }) });
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    for (const box of within(dialog.getByRole("group", { name: "Files" })).getAllByRole("checkbox").slice(0, MAX_ADDITION_FILES)) fireEvent.click(box);
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeEnabled();
    fireEvent.click(within(dialog.getByRole("group", { name: "Files" })).getAllByRole("checkbox")[MAX_ADDITION_FILES]!);
    expect(dialog.getByRole("alert")).toHaveTextContent(`at most ${MAX_ADDITION_FILES}`);
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeDisabled();
  });

  it("keeps everything entered when the add fails, and says why", async () => {
    const onSubmit = vi.fn(async () => { throw new Error("This note changed. Open it before adding to it."); });
    const { dialog, onClose } = renderDialog({ onSubmit, message: message({ attachments: [file("a2", "report.pdf", "application/pdf")] }) });
    fireEvent.click(dialog.getByRole("checkbox", { name: /report.pdf/ }));
    fireEvent.click(dialog.getByRole("checkbox", { name: "Include message text" }));
    fireEvent.click(await dialog.findByRole("option", { name: /Roadmap/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    expect(await dialog.findByRole("alert")).toHaveTextContent("This note changed");
    expect(onClose).not.toHaveBeenCalled();
    expect(dialog.getByRole("checkbox", { name: /report.pdf/ })).toBeChecked();
    expect(dialog.getByRole("checkbox", { name: "Include message text" })).toBeChecked();
    expect(dialog.getByRole("option", { name: /Roadmap/ })).toHaveAttribute("aria-selected", "true");
    expect(dialog.getByRole("button", { name: "Add to Note" })).toBeEnabled();
  });

  it("creates a meeting's note on submit, from the event", async () => {
    vi.spyOn(mailClient, "listScheduleEvents").mockResolvedValue({ events: [meeting("later", "Planning", 26)], errors: [] });
    vi.spyOn(mailClient, "listContactProfiles").mockResolvedValue([]);
    vi.spyOn(mailClient, "resolveContactIds").mockResolvedValue({});
    const { dialog, onSubmit } = renderDialog({ calendarConnected: true });
    fireEvent.click(await dialog.findByRole("option", { name: /Create a note for “Planning”/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledOnce());
    const [{ choice }] = onSubmit.mock.calls[0]! as unknown as [{ choice: { kind: string; date: string; entry: { name: string; linkNote: boolean } } }];
    expect(choice).toMatchObject({ kind: "meeting", date: "2026-11-13", entry: { name: "Planning" } });
  });
});
