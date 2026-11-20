import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Note, NoteLinks } from "../../src/domain";
import { mobile } from "./api";
import { NoteScreen } from "./NoteScreen";

const target = { kind: "note", id: "library" } as const;
const note: Note = {
  id: target.id, title: "Library planning", body: "Bring drawings", dailyDate: null,
  createdAt: "2026-11-20T00:00:00Z", updatedAt: "2026-11-20T00:00:00Z", trashed: false, trashedAt: null,
};
afterEach(() => vi.restoreAllMocks());

describe("mobile note controls", () => {
  it("restores a note through Undo after moving it to Trash from the menu", async () => {
    let current = note;
    vi.spyOn(mobile, "draft").mockResolvedValue(null);
    vi.spyOn(mobile, "note").mockImplementation(async () => ({ note: current, document: null, conflicts: [], links: null }));
    const action = vi.spyOn(mobile, "noteAction").mockImplementation(async (_id, next) => {
      current = { ...note, trashed: next === "trash" };
      return current;
    });

    render(<NoteScreen target={target} revision={0} onOpen={vi.fn()} onChange={vi.fn()} />);
    fireEvent.click(await screen.findByLabelText("Note actions"));
    fireEvent.click(screen.getByRole("button", { name: "Move to Trash" }));
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    expect(await screen.findByRole("button", { name: "Edit" })).toBeVisible();
    expect(action.mock.calls).toEqual([[note.id, "trash"], [note.id, "restore"]]);
  });

  it("shows the number of backlinks and opens their source note", async () => {
    const links = { backlinks: [{ noteId: "other-note", title: "Garden", context: "Library visit" }] } as NoteLinks;
    vi.spyOn(mobile, "draft").mockResolvedValue(null);
    vi.spyOn(mobile, "note").mockResolvedValue({ note, document: null, conflicts: [], links });
    const onOpen = vi.fn();
    render(<NoteScreen target={target} revision={0} onOpen={onOpen} onChange={vi.fn()} />);
    fireEvent.click(await screen.findByText(/^Backlinks/, { selector: "summary" }));
    expect(screen.getByText(/^Backlinks/, { selector: "summary" })).toHaveTextContent("1");
    fireEvent.click(screen.getByRole("button", { name: /Garden/ }));
    expect(onOpen).toHaveBeenCalledWith({ kind: "note", id: "other-note" });
  });
});
