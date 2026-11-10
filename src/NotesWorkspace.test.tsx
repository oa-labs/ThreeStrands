import { StrictMode } from "react";
import { useWorkspaces } from "./useWorkspaces";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installTestShortcuts } from "./test/commandContext";
import { runWorkspaceCommand } from "./workspaceCommands";
import { mailClient } from "./data/client";
import { NotesWorkspace, relinkMessage } from "./NotesWorkspace";
import { lastEditorProps } from "./test/NoteEditorStub";
import { formatNoteDate } from "./noteDates";
import { noteTitleKey as noteTitleKeyOf } from "./noteWikiLinks";
import type { ContactProfile, Note } from "./domain";
import { todayTarget, writeLastNoteTarget } from "./noteDrafts";
import { markdownEntityLink } from "./noteEntities";
import { replicatedSyncConflicts, replicatedSyncDeviceRoster, replicatedSyncResolveConflict, type FrontierConflict } from "./replicatedSync";

// The editor has its own tests; here a text box stands in for it.
vi.mock("./NoteEditor", () => import("./test/NoteEditorStub"));
vi.mock("./replicatedSync", async (original) => ({
  ...await original<typeof import("./replicatedSync")>(),
  replicatedSyncConflicts: vi.fn(async () => []),
  replicatedSyncDeviceRoster: vi.fn(async () => []),
  replicatedSyncResolveConflict: vi.fn(async () => {}),
}));

let counter = 0;
const uniqueTitle = (label: string) => `${label} ${++counter}`;

function shortcuts(overrides = {}) {
  return installTestShortcuts({ focusedPane: "notes", focusSearch: () => runWorkspaceCommand("notes.search"), ...overrides });
}

const noteList = () => within(screen.getByRole("list", { name: "Notes" }));

describe("Notes workspace", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.mocked(replicatedSyncConflicts).mockResolvedValue([]); localStorage.clear(); });

  it("converts checklists and tasks explicitly while retaining item identity", async () => {
    const note = await mailClient.createNote(uniqueTitle("Mixed items"), "- [ ] Local only\n- (x) Shared task\n");
    const original = (await mailClient.listNoteTasks()).find((task) => task.noteId === note.id)!;
    render(<NotesWorkspace request={{ kind: "note", id: 1, noteId: note.id }} />);
    await screen.findByRole("region", { name: "Checklists and tasks" });
    const context = () => within(screen.getByRole("region", { name: "Checklists and tasks" }));
    fireEvent.click(context().getByRole("checkbox", { name: "Checklist: Local only" }));
    await waitFor(async () => expect((await mailClient.getNote(note.id))?.body).toContain("- [x] Local only"));
    expect((await mailClient.listNoteTasks()).filter((task) => task.noteId === note.id)).toHaveLength(1);
    await waitFor(() => expect(context().getByRole("button", { name: "Convert to Task" })).toBeEnabled());
    fireEvent.click(context().getByRole("button", { name: "Convert to Task" }));
    await waitFor(async () => expect((await mailClient.listNoteTasks()).filter((task) => task.noteId === note.id)).toHaveLength(2));
    const promoted = (await mailClient.listNoteTasks()).find((task) => task.noteId === note.id && task.title === "Local only")!;
    expect(promoted.checked).toBe(true);
    await waitFor(() => expect(context().getAllByRole("button", { name: "Convert to Checklist" })[0]).toBeEnabled());
    fireEvent.click(context().getAllByRole("button", { name: "Convert to Checklist" })[0]);
    await waitFor(async () => expect((await mailClient.listNoteTasks()).filter((task) => task.noteId === note.id)).toHaveLength(1));
    expect((await mailClient.listNoteTasks()).find((task) => task.noteId === note.id)?.markerId).toBe(original.markerId);
    await waitFor(() => expect(context().getByRole("button", { name: "Convert to Task" })).toBeEnabled());
    await waitFor(() => expect(context().getByRole("button", { name: "Convert to Task" })).toBeEnabled());
    fireEvent.click(context().getByRole("button", { name: "Convert to Task" }));
    await waitFor(async () => expect((await mailClient.listNoteTasks()).find((task) => task.noteId === note.id && task.title === "Local only")?.markerId).toBe(promoted.markerId));
  });

  it("opens on today's note without taking focus or writing anything", async () => {
    const update = vi.spyOn(mailClient, "updateNote");
    const daily = vi.spyOn(mailClient, "saveDailyNote");
    const create = vi.spyOn(mailClient, "createNote");
    render(<NotesWorkspace />);
    const today = await screen.findByRole("button", { name: /^Today/ });
    expect(today).toHaveTextContent(formatNoteDate(todayTarget().date));
    expect(today).toHaveAttribute("aria-current", "true");
    expect(await screen.findByRole("textbox", { name: "Note text" })).toHaveValue("");
    expect(screen.getByRole("heading", { level: 2, name: formatNoteDate(todayTarget().date) })).toBeInTheDocument();
    expect(document.body).toHaveFocus();
    expect(update).not.toHaveBeenCalled();
    expect(daily).not.toHaveBeenCalled();
    expect(create).not.toHaveBeenCalled();
  });

  it("moves through the list with j and k, writes after Enter, and returns with Escape", async () => {
    const first = await mailClient.createNote(uniqueTitle("Alpha"), "first body");
    const second = await mailClient.createNote(uniqueTitle("Beta"), "second body");
    const openNotes = vi.fn();
    shortcuts({ openNotesView: openNotes, openContactsView: vi.fn() });
    render(<NotesWorkspace />);
    const today = await screen.findByRole("button", { name: /^Today/ });
    await waitFor(() => expect(noteList().getByRole("button", { name: new RegExp(second.title) })).toBeInTheDocument());
    today.focus();
    fireEvent.keyDown(today, { key: "j" });
    const secondRow = noteList().getByRole("button", { name: new RegExp(second.title) });
    expect(secondRow).toHaveFocus();
    expect(secondRow).toHaveAttribute("aria-current", "true");
    expect(await screen.findByRole("textbox", { name: "Note text" })).toHaveValue("second body");
    fireEvent.keyDown(secondRow, { key: "ArrowDown" });
    const firstRow = noteList().getByRole("button", { name: new RegExp(first.title) });
    expect(firstRow).toHaveFocus();
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue("first body"));
    fireEvent.keyDown(firstRow, { key: "k" });
    expect(secondRow).toHaveFocus();

    // "Saved" confirms a save; it doesn't greet every note opened.
    expect(screen.queryByText("Saved")).not.toBeInTheDocument();
    fireEvent.keyDown(secondRow, { key: "Enter" });
    const body = screen.getByRole("textbox", { name: "Note text" });
    await waitFor(() => expect(body).toHaveFocus());
    // Printable keys belong to the note while writing.
    fireEvent.keyDown(body, { key: "4" });
    fireEvent.keyDown(body, { key: "N", shiftKey: true });
    expect(openNotes).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.change(body, { target: { value: "second body, edited" } });
    fireEvent.keyDown(body, { key: "Escape" });
    expect(secondRow).toHaveFocus();
    await waitFor(async () => expect((await mailClient.getNote(second.id))?.body).toBe("second body, edited"));
    await waitFor(() => expect(screen.getByText("Saved")).toHaveAttribute("role", "status"));
  });

  it("says Saved after a save, then clears it", async () => {
    const note = await mailClient.createNote(uniqueTitle("Quiet"), "");
    render(<NotesWorkspace request={{ kind: "note", id: 1, noteId: note.id }} />);
    const body = await screen.findByRole("textbox", { name: "Note text" });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
    // The clock moves only when the test moves it, so a busy machine can't clear "Saved" early.
    vi.useFakeTimers();
    try {
      fireEvent.change(body, { target: { value: "typed" } });
      fireEvent.blur(body);
      await act(() => vi.advanceTimersByTimeAsync(0));
      expect(screen.getByText("Saved")).toHaveAttribute("role", "status");
      await act(() => vi.advanceTimersByTimeAsync(1999));
      expect(screen.getByText("Saved")).toBeInTheDocument();
      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(screen.queryByText("Saved")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it("pages the open note with Space and starts writing with Enter, as reading a message does", async () => {
    const note = await mailClient.createNote(uniqueTitle("Long read"), "many lines");
    const { container } = render(<NotesWorkspace request={{ kind: "note", id: 1, noteId: note.id }} />);
    const row = await noteList().findByRole("button", { name: new RegExp(note.title) });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
    const pane = container.querySelector<HTMLElement>(".notes-editor-pane")!;
    Object.defineProperty(pane, "clientHeight", { value: 500 });
    const scrollBy = vi.fn();
    pane.scrollBy = scrollBy as typeof pane.scrollBy;

    row.focus();
    fireEvent.keyDown(row, { key: " ", code: "Space" });
    expect(scrollBy).toHaveBeenLastCalledWith(expect.objectContaining({ top: 450 }));
    fireEvent.keyDown(row, { key: " ", code: "Space", shiftKey: true });
    expect(scrollBy).toHaveBeenLastCalledWith(expect.objectContaining({ top: -450 }));
    expect(row).toHaveFocus();
    // With nothing focused, the app's shortcuts run the same commands.
    act(() => runWorkspaceCommand("notes.pageDown"));
    expect(scrollBy).toHaveBeenLastCalledWith(expect.objectContaining({ top: 450 }));
    act(() => runWorkspaceCommand("notes.write"));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveFocus());
  });

  it("creates a note from the New Note dialog and opens it for writing", async () => {
    const taken = await mailClient.createNote(uniqueTitle("Taken"), "");
    shortcuts();
    render(<NotesWorkspace />);
    await screen.findByRole("button", { name: /^Today/ });
    fireEvent.keyDown(document.body, { key: "N", shiftKey: true });
    const dialog = await screen.findByRole("dialog", { name: "New Note" });
    const title = within(dialog).getByRole("textbox", { name: "Title" });
    expect(title).toHaveFocus();
    expect(within(dialog).getByRole("button", { name: "Create Note" })).toBeDisabled();

    fireEvent.change(title, { target: { value: taken.title.toUpperCase() } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Note" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("already exists");
    expect(title).toHaveValue(taken.title.toUpperCase());
    expect(title).toHaveAccessibleDescription(/already exists/);

    const name = uniqueTitle("Fresh");
    fireEvent.change(title, { target: { value: name } });
    fireEvent.keyDown(title, { key: "Enter", metaKey: true });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(name);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveFocus());
    expect(noteList().getByRole("button", { name: new RegExp(name) })).toHaveAttribute("aria-current", "true");
  });

  it("closes the New Note dialog without creating anything", async () => {
    const create = vi.spyOn(mailClient, "createNote");
    render(<NotesWorkspace />);
    const newNote = await screen.findByRole("button", { name: "New Note" });
    newNote.focus();
    fireEvent.click(newNote);
    const dialog = await screen.findByRole("dialog", { name: "New Note" });
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Title" }), { target: { value: "Never" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(newNote).toHaveFocus();
    expect(create).not.toHaveBeenCalled();
  });

  it("searches note text from / and returns to the list with Escape", async () => {
    const note = await mailClient.createNote(uniqueTitle("Budget"), "Quarterly forecast numbers");
    shortcuts();
    render(<NotesWorkspace />);
    await screen.findByRole("button", { name: /^Today/ });
    fireEvent.keyDown(document.body, { key: "/" });
    const search = screen.getByRole("searchbox", { name: "Search Notes" });
    expect(search.closest("label")).toHaveClass("search-box");
    await waitFor(() => expect(search).toHaveFocus());
    fireEvent.change(search, { target: { value: "forecast" } });
    const results = within(await screen.findByRole("list", { name: "Matching notes" }));
    expect(results.getByRole("button", { name: new RegExp(note.title) })).toBeInTheDocument();
    expect(results.queryByRole("button", { name: /^Today/ })).not.toBeInTheDocument();
    fireEvent.keyDown(search, { key: "Escape" });
    expect(search).toHaveValue("");
    fireEvent.keyDown(search, { key: "Escape" });
    expect(screen.getByRole("button", { name: /^Today/ })).toHaveFocus();
  });

  it.each(["today", "note"] as const)("restores the last selected note after consuming a %s request and remounting", async (kind) => {
    const requested = await mailClient.createNote(uniqueTitle("Requested"), "requested body");
    const last = await mailClient.createNote(uniqueTitle("Last selected"), "last body");
    function Harness() {
      const workspaces = useWorkspaces({ settingsOpen: false, settingsSection: "appearance", visibleDetail: null,
        readingMessageId: null, readingMessageIdRef: { current: null }, openSettingsAt: vi.fn(), setNotice: vi.fn() });
      return <>
        <button onClick={() => kind === "today" ? workspaces.openTodaysNote() : workspaces.openNote(requested.id)}>Request note</button>
        <button onClick={() => workspaces.setRightWorkspace(null)}>Leave Notes</button>
        <button onClick={workspaces.openNotesView}>Return to Notes</button>
        {workspaces.rightWorkspace === "notes" ? <NotesWorkspace request={workspaces.notesRequest} onRequestConsumed={workspaces.consumeNotesRequest} /> : null}
      </>;
    }
    render(<StrictMode><Harness /></StrictMode>);
    fireEvent.click(screen.getByRole("button", { name: "Request note" }));
    if (kind === "today") {
      await waitFor(() => expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true"));
    } else {
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(requested.title));
    }
    fireEvent.click(await noteList().findByRole("button", { name: new RegExp(last.title) }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(last.title));
    fireEvent.click(screen.getByRole("button", { name: "Leave Notes" }));
    expect(screen.queryByRole("textbox", { name: "Note text" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Return to Notes" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(last.title));
    expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue("last body");
    // A new explicit request still works while Notes is already mounted.
    fireEvent.click(screen.getByRole("button", { name: "Request note" }));
    await waitFor(() => kind === "today"
      ? expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true")
      : expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(requested.title));
  });

  it("goes to today's note when asked from elsewhere", async () => {
    const note = await mailClient.createNote(uniqueTitle("Elsewhere"), "");
    writeLastNoteTarget({ kind: "note", id: note.id });
    const { rerender } = render(<NotesWorkspace request={null} />);
    expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(note.title);
    rerender(<NotesWorkspace request={{ kind: "today", id: 1 }} />);
    expect(await screen.findByRole("textbox", { name: "Note text" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
  });

  it("adds pasted files to the open note, today's included, and says why one couldn't be added", async () => {
    const note = await mailClient.createNote(uniqueTitle("With files"), "");
    const add = vi.spyOn(mailClient, "addNoteAttachment").mockResolvedValue({ id: "0123456789abcdef0123456789abcdef", noteId: note.id, name: "a.png", mimeType: "image/png", size: 3, image: true, available: true });
    render(<NotesWorkspace />);
    await screen.findByRole("textbox", { name: "Note text" });
    const file = new File(["x"], "a.png", { type: "image/png" });
    // Today's note takes a file before it is first saved.
    expect(await lastEditorProps.current!.files!.save(file)).toBe("attachment:0123456789abcdef0123456789abcdef");
    expect(add).toHaveBeenLastCalledWith(`daily:${todayTarget().date}`, file);

    fireEvent.click(await screen.findByRole("button", { name: new RegExp(note.title) }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note.title));
    await lastEditorProps.current!.files!.save(file);
    expect(add).toHaveBeenLastCalledWith(note.id, file);

    add.mockRejectedValue(new Error("Adding files to notes needs the ThreeStrands app"));
    expect(await lastEditorProps.current!.files!.save(file)).toBeUndefined();
    expect(await screen.findByRole("alert")).toHaveTextContent("needs the ThreeStrands app");
  });

  it("asks before replacing a version saved on another device", async () => {
    const note = await mailClient.createNote(uniqueTitle("Shared"), "v1");
    writeLastNoteTarget({ kind: "note", id: note.id });
    render(<NotesWorkspace />);
    const body = await screen.findByRole("textbox", { name: "Note text" });
    await waitFor(() => expect(body).toHaveValue("v1"));
    fireEvent.change(body, { target: { value: "mine" } });
    await mailClient.updateNote(note.id, note.title, "theirs", note.updatedAt);
    act(() => { body.blur(); });
    const banner = await screen.findByRole("alert");
    expect(banner).toHaveTextContent("changed on another device");
    expect(body).toHaveAttribute("readonly");
    expect(body).toHaveValue("mine");
    fireEvent.click(within(banner).getByRole("button", { name: "Keep My Version" }));
    await waitFor(async () => expect((await mailClient.getNote(note.id))?.body).toBe("mine"));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(body).not.toHaveAttribute("readonly");
  });
  it("offers to restore a note moved to Trash on another device while typing", async () => {
    const note = await mailClient.createNote(uniqueTitle("Removed there"), "v1");
    writeLastNoteTarget({ kind: "note", id: note.id });
    render(<NotesWorkspace />);
    const body = await screen.findByRole("textbox", { name: "Note text" });
    await waitFor(() => expect(body).toHaveValue("v1"));
    fireEvent.change(body, { target: { value: "mine" } });
    await mailClient.trashNote(note.id);
    act(() => { body.blur(); });
    const banner = await screen.findByRole("alert");
    expect(banner).toHaveTextContent("moved to Trash on another device");
    expect(within(banner).getByRole("button", { name: "Discard My Text" })).toBeInTheDocument();
    fireEvent.click(within(banner).getByRole("button", { name: "Restore and Keep My Text" }));
    await waitFor(async () => expect(await mailClient.getNote(note.id)).toMatchObject({ trashed: false, body: "mine" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(body).not.toHaveAttribute("readonly");
  });
  describe("Trash", () => {
    it("moves the open note to Trash with # and opens the next note", async () => {
      const kept = await mailClient.createNote(uniqueTitle("Kept"), "stays");
      const doomed = await mailClient.createNote(uniqueTitle("Doomed"), "goes");
      await mailClient.recordNoteVisit(kept.id);
      await mailClient.recordNoteVisit(doomed.id);
      writeLastNoteTarget({ kind: "note", id: doomed.id });
      shortcuts();
      render(<NotesWorkspace />);
      expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(doomed.title);
      fireEvent.keyDown(document.body, { key: "#", shiftKey: true, code: "Digit3" });
      await waitFor(() => expect(noteList().queryByRole("button", { name: new RegExp(doomed.title) })).not.toBeInTheDocument());
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(kept.title));
      expect((await mailClient.getNote(doomed.id))?.trashed).toBe(true);
      expect(screen.getByRole("button", { name: /^Trash/ })).toHaveTextContent(/\d/);
    });

    it("keeps Today open when its note goes to Trash, and leaves the day empty once deleted", async () => {
      const today = todayTarget();
      await mailClient.saveDailyNote(today.date, "written today", (await mailClient.getDailyNote(today.date))?.updatedAt ?? null);
      shortcuts();
      render(<NotesWorkspace />);
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue("written today"));
      fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
      const banner = within(await screen.findByRole("region", { name: "Note in Trash" }));
      expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
      fireEvent.click(banner.getByRole("button", { name: "Delete Permanently" }));
      fireEvent.click(within(banner.getByRole("group", { name: "Delete note permanently" })).getByRole("button", { name: "Delete Permanently" }));
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Note text" })).toHaveValue(""));
      expect(screen.queryByRole("region", { name: "Note in Trash" })).not.toBeInTheDocument();
      expect(screen.getByRole("textbox", { name: "Note text" })).not.toHaveAttribute("readonly");
    });

    it("lists trashed notes, opens them read-only, and restores them", async () => {
      const note = await mailClient.createNote(uniqueTitle("Restorable"), "kept text");
      await mailClient.trashNote(note.id);
      render(<NotesWorkspace />);
      fireEvent.click(await screen.findByRole("button", { name: /^Trash/ }));
      const trash = within(await screen.findByRole("list", { name: "Trash" }));
      fireEvent.click(await trash.findByRole("button", { name: new RegExp(note.title) }));
      const banner = await screen.findByRole("region", { name: "Note in Trash" });
      expect(screen.getByRole("textbox", { name: "Note text" })).toHaveAttribute("readonly");
      expect(screen.getByRole("textbox", { name: "Title" })).toHaveAttribute("readonly");
      expect(screen.queryByRole("button", { name: "Move to Trash" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Link to…" })).not.toBeInTheDocument();
      act(() => runWorkspaceCommand("notes.write"));
      await act(() => new Promise(requestAnimationFrame));
      expect(screen.getByRole("textbox", { name: "Note text" })).not.toHaveFocus();

      fireEvent.click(within(banner).getByRole("button", { name: "Restore" }));
      await waitFor(() => expect(screen.queryByRole("region", { name: "Note in Trash" })).not.toBeInTheDocument());
      expect(screen.getByRole("textbox", { name: "Note text" })).not.toHaveAttribute("readonly");
      expect((await mailClient.getNote(note.id))?.trashed).toBe(false);
      expect(trash.queryByRole("button", { name: new RegExp(note.title) })).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Back to Notes" }));
      expect(noteList().getByRole("button", { name: new RegExp(note.title) })).toBeInTheDocument();
    });

    it("explains a refused restore", async () => {
      const note = await mailClient.createNote(uniqueTitle("Twice"), "");
      await mailClient.trashNote(note.id);
      await mailClient.createNote(note.title, "");
      writeLastNoteTarget({ kind: "note", id: note.id });
      render(<NotesWorkspace />);
      fireEvent.click(within(await screen.findByRole("region", { name: "Note in Trash" })).getByRole("button", { name: "Restore" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Rename that note");
      expect((await mailClient.getNote(note.id))?.trashed).toBe(true);
    });

    it("deletes permanently only after confirming, starting on Cancel", async () => {
      const note = await mailClient.createNote(uniqueTitle("Forever"), "");
      await mailClient.trashNote(note.id);
      writeLastNoteTarget({ kind: "note", id: note.id });
      const remove = vi.spyOn(mailClient, "deleteNote");
      render(<NotesWorkspace />);
      const banner = await screen.findByRole("region", { name: "Note in Trash" });
      fireEvent.click(within(banner).getByRole("button", { name: "Delete Permanently" }));
      const confirm = within(banner).getByRole("group", { name: "Delete note permanently" });
      expect(within(confirm).getByRole("button", { name: "Cancel" })).toHaveFocus();
      fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
      expect(remove).not.toHaveBeenCalled();

      fireEvent.click(within(banner).getByRole("button", { name: "Delete Permanently" }));
      fireEvent.click(within(within(banner).getByRole("group", { name: "Delete note permanently" })).getByRole("button", { name: "Delete Permanently" }));
      await waitFor(() => expect(remove).toHaveBeenCalledWith(note.id));
      expect(await mailClient.getNote(note.id)).toBeNull();
      await waitFor(() => expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true"));
    });
  });

  describe("conflicts", () => {
    const candidates = (values: [string, unknown][]) => values.map(([deviceId, value], index) => ({ operationId: `${deviceId}:${index + 1}`, deviceId, value }));
    const conflictOn = (id: string, field: string, values: [string, unknown][]): FrontierConflict => ({ entityType: "note", entityId: id, field, candidates: candidates(values) });

    async function conflictedNote() {
      return conflictedNoteFor(await mailClient.createNote(uniqueTitle("Shared plan"), "line one\nmine"));
    }

    async function conflictedNoteFor(note: Note) {
      await mailClient.recordNoteVisit(note.id);
      const conflicts = [conflictOn(note.id, "body", [["self", "line one\nmine"], ["laptop", "line one\ntheirs"]])];
      vi.mocked(replicatedSyncConflicts).mockResolvedValue(conflicts);
      vi.mocked(replicatedSyncDeviceRoster).mockResolvedValue([
        { deviceId: "self", status: "active", isSelf: true, label: null },
        { deviceId: "laptop", status: "active", isSelf: false, label: "Work laptop" },
      ]);
      writeLastNoteTarget({ kind: "note", id: note.id });
      render(<NotesWorkspace />);
      const banner = await screen.findByRole("region", { name: "Note conflict" });
      return { note, conflicts, banner: within(banner) };
    }

    it("names the other device, marks the note, compares, and keeps the note read-only", async () => {
      const { note, banner } = await conflictedNote();
      await waitFor(() => expect(screen.getByRole("region", { name: "Note conflict" })).toHaveTextContent("Work laptop"));
      expect(noteList().getByRole("button", { name: new RegExp(note.title) })).toHaveTextContent("Changed on two devices");
      expect(screen.getByRole("textbox", { name: "Note text" })).toHaveAttribute("readonly");
      expect(screen.getByRole("textbox", { name: "Title" })).toHaveAttribute("readonly");

      fireEvent.click(banner.getByRole("button", { name: "Compare" }));
      const changes = within(screen.getByRole("list", { name: /Changes from this version/ }));
      expect(changes.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
        " line one",
        "Only in this version: −mine",
        "Only from Work laptop: +theirs",
      ]);
      expect(banner.getByRole("button", { name: "Hide Comparison" })).toHaveAttribute("aria-expanded", "true");
    });

    it("keeps the other version by choosing its value for each field", async () => {
      const { conflicts, banner } = await conflictedNote();
      fireEvent.click(banner.getByRole("button", { name: "Use Other Version" }));
      await waitFor(() => expect(replicatedSyncResolveConflict).toHaveBeenCalledWith(conflicts[0], conflicts[0]!.candidates[1]));
    });

    it("keeps both by saving the other version as its own note first", async () => {
      const { note, conflicts, banner } = await conflictedNote();
      await waitFor(() => expect(screen.getByRole("region", { name: "Note conflict" })).toHaveTextContent("Work laptop"));
      const create = vi.spyOn(mailClient, "createNote");
      fireEvent.click(banner.getByRole("button", { name: "Keep Both" }));
      await waitFor(() => expect(replicatedSyncResolveConflict).toHaveBeenCalledWith(conflicts[0], conflicts[0]!.candidates[0]));
      expect(create).toHaveBeenCalledWith(`${note.title} (from Work laptop)`, "line one\ntheirs");
      expect(create.mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(replicatedSyncResolveConflict).mock.invocationCallOrder.at(-1)!);
    });

    it("keeps both with a long title shortened to fit, and a name in use numbered", async () => {
      const long = "Quarterly planning ".repeat(11).trim().slice(0, 200);
      const note = await mailClient.createNote(long, "line one\nmine");
      const { conflicts } = await conflictedNoteFor(note);
      await waitFor(() => expect(screen.getByRole("region", { name: "Note conflict" })).toHaveTextContent("Work laptop"));
      const taken = await mailClient.createNote(`${long.slice(0, 200 - " (from Work laptop)".length).trimEnd()} (from Work laptop)`, "");
      const create = vi.spyOn(mailClient, "createNote");
      fireEvent.click(within(screen.getByRole("region", { name: "Note conflict" })).getByRole("button", { name: "Keep Both" }));
      await waitFor(() => expect(replicatedSyncResolveConflict).toHaveBeenCalledWith(conflicts[0], conflicts[0]!.candidates[0]));
      expect(create).toHaveBeenCalledTimes(1);
      const [name] = create.mock.calls[0]!;
      expect(name).not.toBe(taken.title);
      expect([...name]).toHaveLength(200);
      expect(name.endsWith(" (from Work laptop) 2")).toBe(true);
    });

    it("finishes Keep Both on a second try without making another copy", async () => {
      const { conflicts, banner } = await conflictedNote();
      await waitFor(() => expect(screen.getByRole("region", { name: "Note conflict" })).toHaveTextContent("Work laptop"));
      const create = vi.spyOn(mailClient, "createNote");
      vi.mocked(replicatedSyncResolveConflict).mockRejectedValueOnce(new Error("Sync is busy"));
      fireEvent.click(banner.getByRole("button", { name: "Keep Both" }));
      expect(await banner.findByRole("alert")).toHaveTextContent("Sync is busy");
      fireEvent.click(banner.getByRole("button", { name: "Keep Both" }));
      await waitFor(() => expect(vi.mocked(replicatedSyncResolveConflict).mock.calls.filter(([conflict]) => conflict === conflicts[0])).toHaveLength(2));
      expect(create).toHaveBeenCalledTimes(1);
      expect(banner.queryByRole("alert")).not.toBeInTheDocument();
    });
  });

  it("opens a note asked for from elsewhere, even from Trash view", async () => {
    const note = await mailClient.createNote(uniqueTitle("Linked"), "");
    const { rerender } = render(<NotesWorkspace request={null} />);
    fireEvent.click(await screen.findByRole("button", { name: /^Trash/ }));
    rerender(<NotesWorkspace request={{ kind: "note", noteId: note.id, id: 1 }} />);
    expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(note.title);
    expect(screen.getByRole("list", { name: "Notes" })).toBeInTheDocument();
  });
  describe("links", () => {
    const rail = () => within(screen.getByRole("complementary", { name: "Note context" }));
    const openNote = async (id: string, props: Partial<Parameters<typeof NotesWorkspace>[0]> = {}) => {
      writeLastNoteTarget({ kind: "note", id });
      render(<NotesWorkspace {...props} />);
      const note = await mailClient.getNote(id);
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(note!.title));
    };

    it("lists backlinks with their line and opens the linking note", async () => {
      const target = await mailClient.createNote(uniqueTitle("Hub"), "");
      const linking = await mailClient.createNote(uniqueTitle("Spoke"), `Intro\nTalked about [[${target.title.toUpperCase()}]] today`);
      await openNote(target.id);
      const section = within(await rail().findByRole("region", { name: /Backlinks/ }));
      const row = section.getByRole("button", { name: new RegExp(linking.title) });
      expect(row).toHaveTextContent(`Talked about [[${target.title.toUpperCase()}]] today`);
      fireEvent.click(row);
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(linking.title));
    });

    it("creates an unlinked note when chosen, and opens a day for a date link", async () => {
      const missing = uniqueTitle("Someday");
      const note = await mailClient.createNote(uniqueTitle("Ideas"), `Next: [[${missing}]] and [[2026-11-10]]`);
      await openNote(note.id);
      const section = within(await rail().findByRole("region", { name: /Unlinked/ }));
      expect(section.getByRole("button", { name: new RegExp(formatNoteDate("2026-11-10")) })).toBeInTheDocument();
      fireEvent.click(section.getByRole("button", { name: new RegExp(missing) }));
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(missing));
      expect(await mailClient.findNoteByTitle(missing)).not.toBeNull();

      act(() => lastEditorProps.current!.onOpenLink!("2026-11-10"));
      await waitFor(() => expect(screen.getByRole("heading", { level: 2, name: formatNoteDate("2026-11-10") })).toBeInTheDocument());
      expect(await mailClient.getDailyNote("2026-11-10")).toBeNull();
    });

    it("follows a link to an existing note by title, ignoring case and spacing", async () => {
      const target = await mailClient.createNote(uniqueTitle("Existing target"), "");
      const note = await mailClient.createNote(uniqueTitle("Source"), "");
      const create = vi.spyOn(mailClient, "createNote");
      await openNote(note.id);
      act(() => lastEditorProps.current!.onOpenLink!(`  ${target.title.toLowerCase().replace(" ", "   ")} `));
      await waitFor(() => expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue(target.title));
      expect(create).not.toHaveBeenCalled();
    });

    it("offers other notes in the link picker", async () => {
      const other = await mailClient.createNote(uniqueTitle("Pickable"), "");
      const note = await mailClient.createNote(uniqueTitle("Pickable self"), "");
      await openNote(note.id);
      // The workspace learns which note is open in an effect after the title
      // shows, so search again until it has.
      await waitFor(async () => {
        const found = (await lastEditorProps.current!.onSearchLinks!("pickable")).map((item) => item.title);
        expect(found).toContain(other.title);
        expect(found).not.toContain(note.title);
      });
    });

    it("lists linked people who are saved contacts and opens them in Contacts", async () => {
      const note = await mailClient.createNote(uniqueTitle("Met"), "Lunch with [[Ada  Lovelace]] and [[Nobody Known]]");
      vi.spyOn(mailClient, "listContactProfiles").mockImplementation(async (query) => noteTitleKeyOf(query ?? "") === "ada lovelace"
        ? [{ id: "c-ada", displayName: "Ada Lovelace", company: "Analytical Engines", addresses: ["ada@example.com"] } as ContactProfile]
        : [{ id: "c-near", displayName: "Nobody Else", company: null, addresses: [] } as unknown as ContactProfile]);
      const onOpenContact = vi.fn();
      await openNote(note.id, { onOpenContact });
      const people = within(await rail().findByRole("region", { name: /People/ }));
      expect(people.queryByRole("button", { name: /Nobody/ })).not.toBeInTheDocument();
      fireEvent.click(people.getByRole("button", { name: /Ada Lovelace/ }));
      expect(onOpenContact).toHaveBeenCalledWith("c-ada");
    });

    it("lists contact links under People rather than Links, once per contact", async () => {
      const pete = markdownEntityLink("Pete B", { version: 1, kind: "contact", id: "pete@example.com", contact: "c-pete" });
      const peteAgain = markdownEntityLink("Pete", { version: 1, kind: "contact", id: "pete@example.com" });
      const frank = markdownEntityLink("Frank", { version: 1, kind: "contact", id: "frank@example.com" });
      const thread = markdownEntityLink("Agenda", { version: 1, kind: "conversation", id: "thread-a", account: "acct-1" });
      const note = await mailClient.createNote(uniqueTitle("Standup"), `- met with ${pete}, ${frank} for ${thread}\n- ${peteAgain}`);
      const resolve = vi.spyOn(mailClient, "resolveNoteContact").mockImplementation(async (reference) => reference.id === "pete@example.com" ? "c-pete" : null);
      vi.spyOn(mailClient, "getContactProfile").mockImplementation(async (id) => id === "c-pete" ? { id: "c-pete", displayName: "Pete Brown", company: null, addresses: ["pete@example.com"] } as unknown as ContactProfile : null);
      const onOpenContact = vi.fn();
      await openNote(note.id, { onOpenContact });
      const people = within(await rail().findByRole("region", { name: /People/ }));
      expect(people.getAllByRole("button", { name: /Pete Brown/ })).toHaveLength(1);
      expect(people.queryByRole("button", { name: /Frank/ })).not.toBeInTheDocument();
      expect(resolve).toHaveBeenCalledTimes(2);
      const links = within(rail().getByRole("region", { name: /Links/ }));
      expect(links.getByRole("button", { name: /Agenda/ })).toBeInTheDocument();
      expect(links.queryByRole("button", { name: /Pete|Frank/ })).not.toBeInTheDocument();
      fireEvent.click(people.getByRole("button", { name: /Pete Brown/ }));
      expect(onOpenContact).toHaveBeenCalledWith("c-pete");
    });

    it("lists what the note links to, and adds a link from the Links section or the command", async () => {
      const href = markdownEntityLink("Quarterly plan", { version: 1, kind: "conversation", id: "thread-q", account: "acct-1" });
      const note = await mailClient.createNote(uniqueTitle("Linked"), `See ${href}`);
      await openNote(note.id);
      const links = within(await rail().findByRole("region", { name: /Links/ }));
      expect(await links.findByRole("button", { name: /Quarterly plan/ })).toBeInTheDocument();
      fireEvent.click(links.getByRole("button", { name: "Link to…" }));
      const picker = await screen.findByRole("dialog", { name: "Link to…" });
      fireEvent.keyDown(picker, { key: "Escape" });
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Link to…" })).not.toBeInTheDocument());
      act(() => runWorkspaceCommand("notes.link"));
      expect(await screen.findByRole("dialog", { name: "Link to…" })).toBeInTheDocument();
    });

    it("saves a renamed title when the field is left and says what happened to links", async () => {
      const target = await mailClient.createNote(uniqueTitle("Before"), "");
      const linking = await mailClient.createNote(uniqueTitle("Mentions"), `See [[${target.title}]]`);
      await openNote(target.id);
      const title = screen.getByRole("textbox", { name: "Title" });
      const renamed = uniqueTitle("After");
      fireEvent.change(title, { target: { value: renamed } });
      fireEvent.blur(title);
      expect(await screen.findByText("Updated links in 1 note.")).toHaveAttribute("role", "status");
      expect((await mailClient.getNote(linking.id))?.body).toBe(`See [[${renamed}]]`);
    });

    it("describes links a rename couldn't update", () => {
      expect(relinkMessage({ updated: 0, keptOldLink: [] })).toBeNull();
      expect(relinkMessage({ updated: 2, keptOldLink: ["Full", "2026-11-09"] })).toBe(
        `Updated links in 2 notes. “Full”, ${formatNoteDate("2026-11-09")} keep the old link, because changing them would make them too long to sync or would settle a conflict.`,
      );
    });
  });
});
