import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installTestShortcuts } from "./test/commandContext";
import { runWorkspaceCommand } from "./workspaceCommands";
import { mailClient } from "./data/client";
import { NotesWorkspace, formatNoteDate } from "./NotesWorkspace";
import { todayTarget, writeLastNoteTarget } from "./noteDrafts";

let counter = 0;
const uniqueTitle = (label: string) => `${label} ${++counter}`;

function shortcuts(overrides = {}) {
  return installTestShortcuts({ focusedPane: "notes", focusSearch: () => runWorkspaceCommand("notes.search"), ...overrides });
}

const noteList = () => within(screen.getByRole("list", { name: "Notes" }));

describe("Notes workspace", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); localStorage.clear(); });

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
});
