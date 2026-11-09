import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installTestShortcuts } from "./test/commandContext";
import { runWorkspaceCommand } from "./workspaceCommands";
import { mailClient } from "./data/client";
import { NotesWorkspace } from "./NotesWorkspace";
import { formatNoteDate } from "./noteDates";
import { todayTarget, writeLastNoteTarget } from "./noteDrafts";
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

  it("goes to today's note when asked from elsewhere", async () => {
    const note = await mailClient.createNote(uniqueTitle("Elsewhere"), "");
    writeLastNoteTarget({ kind: "note", id: note.id });
    const { rerender } = render(<NotesWorkspace request={null} />);
    expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(note.title);
    rerender(<NotesWorkspace request={{ kind: "today", id: 1 }} />);
    expect(await screen.findByRole("textbox", { name: "Note text" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
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
      const note = await mailClient.createNote(uniqueTitle("Shared plan"), "line one\nmine");
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
  });

  it("opens a note asked for from elsewhere, even from Trash view", async () => {
    const note = await mailClient.createNote(uniqueTitle("Linked"), "");
    const { rerender } = render(<NotesWorkspace request={null} />);
    fireEvent.click(await screen.findByRole("button", { name: /^Trash/ }));
    rerender(<NotesWorkspace request={{ kind: "note", noteId: note.id, id: 1 }} />);
    expect(await screen.findByRole("textbox", { name: "Title" })).toHaveValue(note.title);
    expect(screen.getByRole("list", { name: "Notes" })).toBeInTheDocument();
  });
});
