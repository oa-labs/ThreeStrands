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

  describe("Import Markdown Folder", () => {
    async function openImport() {
      render(<SettingsHarness initialSection="notes" />);
      const dialog = await screen.findByRole("dialog", { name: "Settings" });
      return within(within(dialog).getByRole("region", { name: "Import notes" }));
    }
    const summary = { path: "/Users/me/Reflect", notes: 406, dailyNotes: 67, files: 44, alreadyHere: 2, problems: [{ file: "person/bethany.md~other device", reason: "A copy Reflect kept of conflicting edits." }] };

    it("shows what a folder holds and imports only after confirming, starting on Cancel", async () => {
      vi.spyOn(mailClient, "chooseNotesImportFolder").mockResolvedValue(summary);
      const run = vi.spyOn(mailClient, "importNotes").mockResolvedValue({ notes: 473, files: 44, problems: [] });
      const section = await openImport();
      const button = section.getByRole("button", { name: "Import Markdown Folder…" });
      expect(button).toHaveAccessibleDescription();
      fireEvent.click(button);
      const confirm = within(await section.findByRole("group", { name: "Import notes" }));
      expect(confirm.getByText("Import 473 notes from /Users/me/Reflect?")).toBeInTheDocument();
      expect(confirm.getByText(/406 notes, 67 daily notes, and 44 files\. 2 notes are already here/)).toBeInTheDocument();
      expect(confirm.getByText("1 file won’t be imported")).toBeInTheDocument();
      expect(confirm.getByText("person/bethany.md~other device")).toBeInTheDocument();
      await waitFor(() => expect(confirm.getByRole("button", { name: "Cancel" })).toHaveFocus());
      fireEvent.click(confirm.getByRole("button", { name: "Cancel" }));
      expect(run).not.toHaveBeenCalled();

      fireEvent.click(button);
      fireEvent.click(within(await section.findByRole("group", { name: "Import notes" })).getByRole("button", { name: "Import Notes" }));
      expect(await section.findByRole("status")).toHaveTextContent("Imported 473 notes and 44 files.");
      expect(run).toHaveBeenCalledTimes(1);
    });

    it("says when a folder has nothing new, and lists notes that couldn't be saved", async () => {
      const choose = vi.spyOn(mailClient, "chooseNotesImportFolder").mockResolvedValue({ ...summary, notes: 0, dailyNotes: 0, files: 0, alreadyHere: 473, problems: [] });
      vi.spyOn(mailClient, "importNotes").mockResolvedValue({ notes: 1, files: 0, problems: [{ file: "notes/Big.md", reason: "Notes have used all of their sync space." }] });
      const section = await openImport();
      fireEvent.click(section.getByRole("button", { name: "Import Markdown Folder…" }));
      expect(await section.findByRole("status")).toHaveTextContent("Every note in /Users/me/Reflect is already in Notes.");
      expect(section.queryByRole("group", { name: "Import notes" })).not.toBeInTheDocument();

      choose.mockResolvedValue({ ...summary, notes: 2, dailyNotes: 0, problems: [] });
      fireEvent.click(section.getByRole("button", { name: "Import Markdown Folder…" }));
      fireEvent.click(within(await section.findByRole("group", { name: "Import notes" })).getByRole("button", { name: "Import Notes" }));
      await waitFor(() => expect(section.getByRole("status")).toHaveTextContent("Imported 1 note."));
      expect(section.getByText("1 file wasn’t imported")).toBeInTheDocument();
      expect(section.getByText("notes/Big.md")).toBeInTheDocument();
    });

    it("does nothing when the picker is cancelled, and shows why a folder couldn't be read", async () => {
      const choose = vi.spyOn(mailClient, "chooseNotesImportFolder").mockResolvedValue(null);
      const section = await openImport();
      fireEvent.click(section.getByRole("button", { name: "Import Markdown Folder…" }));
      await waitFor(() => expect(choose).toHaveBeenCalled());
      expect(section.queryByRole("group")).not.toBeInTheDocument();
      expect(section.queryByRole("status")).not.toBeInTheDocument();
      choose.mockRejectedValue(new Error("This folder holds more than 20000 files. Choose the folder that holds your notes."));
      fireEvent.click(section.getByRole("button", { name: "Import Markdown Folder…" }));
      expect(await section.findByRole("alert")).toHaveTextContent("more than 20000 files");
    });
  });

  describe("Export to Markdown Folder", () => {
    async function openNotes() {
      render(<SettingsHarness initialSection="notes" />);
      const dialog = await screen.findByRole("dialog", { name: "Settings" });
      return within(within(dialog).getByRole("region", { name: "Export notes" }));
    }

    it("exports straight away to an empty folder and says where", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Notes", state: "empty" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes").mockResolvedValue({ path: "/Users/me/Notes", notes: 12, attachments: 0, missingAttachments: 0 });
      const section = await openNotes();
      const button = section.getByRole("button", { name: "Export to Markdown Folder…" });
      expect(button).toHaveAccessibleDescription();
      fireEvent.click(button);
      expect(await section.findByRole("status")).toHaveTextContent("Exported 12 notes to /Users/me/Notes.");
      expect(exportNotes).toHaveBeenCalledWith(false);
    });

    it("asks before replacing an earlier export, starting on Cancel", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Notes", state: "earlierExport" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes").mockResolvedValue({ path: "/Users/me/Notes", notes: 1, attachments: 0, missingAttachments: 0 });
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

    it("counts the files it wrote and the ones this device doesn't have yet", async () => {
      vi.spyOn(mailClient, "chooseNotesExportFolder").mockResolvedValue({ path: "/Users/me/Notes", state: "empty" });
      const exportNotes = vi.spyOn(mailClient, "exportNotes").mockResolvedValue({ path: "/Users/me/Notes", notes: 3, attachments: 2, missingAttachments: 1 });
      const section = await openNotes();
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      expect(await section.findByRole("status")).toHaveTextContent("Exported 3 notes and 2 files to /Users/me/Notes. 1 file hasn't reached this device yet, so it was left out.");
      exportNotes.mockResolvedValue({ path: "/Users/me/Notes", notes: 1, attachments: 0, missingAttachments: 2 });
      fireEvent.click(section.getByRole("button", { name: "Export to Markdown Folder…" }));
      await waitFor(() => expect(section.getByRole("status")).toHaveTextContent("Exported 1 note to /Users/me/Notes. 2 files haven't reached this device yet, so they were left out."));
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
    vi.spyOn(mailClient, "notesStorage").mockResolvedValue({ usedBytes: 1536, limitBytes: 3 * 1024 * 1024, attachmentCount: 0, attachmentBytes: 0 });
    render(<SettingsHarness initialSection="notes" />);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const region = await within(dialog).findByRole("region", { name: "Notes storage" });
    expect(region).toHaveTextContent("Notes use 2 KB of the 3.0 MB");
    expect(region).not.toHaveTextContent("files");
  });

  it("counts attached files apart from the notes' sync space", async () => {
    vi.spyOn(mailClient, "notesStorage").mockResolvedValue({ usedBytes: 1536, limitBytes: 3 * 1024 * 1024, attachmentCount: 4, attachmentBytes: 5 * 1024 * 1024 });
    render(<SettingsHarness initialSection="notes" />);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(await within(dialog).findByRole("region", { name: "Notes storage" })).toHaveTextContent("Their 4 files take 5.0 MB more.");
  });
});
