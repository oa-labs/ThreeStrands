import { act, cleanup, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import { SettingsHarness } from "./test/settingsHarness";
import { readNoteSyntaxMode, useNoteSyntaxMode, writeNoteSyntaxMode } from "./noteSyntaxMode";

describe("Notes settings", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("starts on Hide and keeps the Markdown Syntax choice on this device", async () => {
    render(<SettingsHarness initialSection="notes" />);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const group = within(dialog).getByRole("radiogroup", { name: "Markdown syntax" });
    expect(within(group).getByRole("radio", { name: "Hide" })).toBeChecked();
    expect(group).toHaveAccessibleDescription();
    fireEvent.click(within(group).getByRole("radio", { name: "Hybrid" }));
    expect(within(group).getByRole("radio", { name: "Hybrid" })).toBeChecked();
    expect(readNoteSyntaxMode()).toBe("hybrid");
  });

  it("tells an open editor when the choice changes, and ignores an unknown stored value", () => {
    localStorage.setItem("threestrands.notes.syntax", "sideways");
    const { result } = renderHook(() => useNoteSyntaxMode());
    expect(result.current).toBe("hide");
    act(() => writeNoteSyntaxMode("show"));
    expect(result.current).toBe("show");
  });

  describe("Export to Markdown Folder", () => {
    async function openNotes() {
      render(<SettingsHarness initialSection="notes" />);
      const dialog = await screen.findByRole("dialog", { name: "Settings" });
      return within(within(dialog).getByRole("region", { name: "Export notes" }));
    }

    it("exports straight away to an empty folder and says where", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Notes", state: "empty" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes").mockResolvedValue({ path: "/Users/me/Notes", notes: 12 });
      const section = await openNotes();
      const button = section.getByRole("button", { name: "Export to Markdown Folder…" });
      expect(button).toHaveAccessibleDescription();
      fireEvent.click(button);
      expect(await section.findByRole("status")).toHaveTextContent("Exported 12 notes to /Users/me/Notes.");
      expect(exportNotes).toHaveBeenCalledWith(false);
    });

    it("asks before replacing an earlier export, starting on Cancel", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Notes", state: "earlierExport" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes").mockResolvedValue({ path: "/Users/me/Notes", notes: 1 });
      const section = await openNotes();
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      const confirm = within(await section.findByRole("group", { name: "Replace earlier export" }));
      expect(confirm.getByText("Replace the earlier export in /Users/me/Notes?")).toBeInTheDocument();
      expect(confirm.getByRole("button", { name: "Cancel" })).toHaveFocus();
      fireEvent.click(confirm.getByRole("button", { name: "Cancel" }));
      expect(exportNotes).not.toHaveBeenCalled();

      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      fireEvent.click(within(await section.findByRole("group", { name: "Replace earlier export" })).getByRole("button", { name: "Replace Export" }));
      await waitFor(() => expect(exportNotes).toHaveBeenCalledWith(true));
      expect(await section.findByRole("status")).toHaveTextContent("Exported 1 note to");
    });

    it("refuses a folder holding other files, and does nothing when the picker is cancelled", async () => {
      const choose = vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Documents", state: "inUse" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes");
      const section = await openNotes();
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      expect(await section.findByRole("alert")).toHaveTextContent("already holds other files");
      choose.mockResolvedValue(null);
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      await waitFor(() => expect(section.queryByRole("alert")).not.toBeInTheDocument());
      expect(exportNotes).not.toHaveBeenCalled();
    });

    it("shows a failed export's reason", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/x", state: "empty" });
      vi.spyOn(mailClient, "exportNotes").mockRejectedValue(new Error("The folder's “notes” is a link or a file, so the export won't write there."));
      const section = await openNotes();
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      expect(await section.findByRole("alert")).toHaveTextContent("is a link or a file");
    });
  });

  it("shows how much of their sync space notes use", async () => {
    vi.spyOn(mailClient, "notesStorage").mockResolvedValue({ usedBytes: 1536, limitBytes: 3 * 1024 * 1024 });
    render(<SettingsHarness initialSection="notes" />);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(await within(dialog).findByRole("region", { name: "Notes storage" })).toHaveTextContent("Notes use 2 KB of the 3.0 MB");
  });
});
