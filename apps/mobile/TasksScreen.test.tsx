import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Note, ThreadTask } from "../../src/domain";
import type { NoteTaskRecord } from "../../src/noteEntities";
import { mobile } from "./api";
import { TasksScreen } from "./TasksScreen";

const task: ThreadTask = {
  id: "task", accountId: "", threadId: null, subjectSnapshot: null, title: "Call the library",
  kind: "action", dueKind: "none", status: "in_progress", createdAt: "created", updatedAt: "original",
};

const props = { revision: 0, onChange: vi.fn(), done: false, setDone: vi.fn(), onEdit: vi.fn(), onOpenNote: vi.fn() };
afterEach(() => vi.restoreAllMocks());

describe("mobile task completion", () => {
  it("offers undo after removing a completed task and restores its prior status", async () => {
    vi.spyOn(mobile, "tasks").mockResolvedValue({ standalone: [task], fromNotes: [] });
    const check = vi.spyOn(mobile, "checkTask")
      .mockResolvedValueOnce({ ...task, status: "completed", updatedAt: "completed" })
      .mockResolvedValueOnce({ ...task, updatedAt: "undone" });
    render(<TasksScreen {...props} />);
    fireEvent.click(await screen.findByRole("checkbox", { name: task.title }));
    const undo = await screen.findByRole("button", { name: "Undo" });
    expect(screen.queryByRole("checkbox", { name: task.title })).toBeNull();
    fireEvent.click(undo);
    expect(await screen.findByRole("checkbox", { name: task.title, checked: false })).toBeVisible();
    expect(check.mock.calls).toEqual([
      [task.id, true, "original", undefined],
      [task.id, false, "completed", "in_progress"],
    ]);
  });

  it("undoes a note task against the version returned by completing it", async () => {
    const item: NoteTaskRecord = {
      id: "from-note", markerId: "marker", noteId: "note", noteTitle: "Garden",
      title: "Order seeds", checked: false, updatedAt: "original", completedAt: null, conflicted: false,
    };
    vi.spyOn(mobile, "tasks").mockResolvedValue({ standalone: [], fromNotes: [item] });
    const check = vi.spyOn(mobile, "checkNoteItem")
      .mockResolvedValueOnce({ updatedAt: "completed" } as Note)
      .mockResolvedValueOnce({ updatedAt: "undone" } as Note);
    render(<TasksScreen {...props} />);
    fireEvent.click(await screen.findByRole("checkbox", { name: item.title }));
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(check).toHaveBeenLastCalledWith("note", "marker", "completed", false));
    expect(await screen.findByRole("checkbox", { name: item.title, checked: false })).toBeVisible();
  });
});
