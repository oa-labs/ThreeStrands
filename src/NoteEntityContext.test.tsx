import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { NoteReference } from "./noteEntities";
import { EventNoteAction, EventNoteBacklinks } from "./NoteEntityContext";
import { notesChanged } from "./noteMutations";

const occurrence: NoteReference = { version: 1, kind: "event", id: "standup_1112", account: "you@example.com", calendar: "work", series: "standup" };
const row = (noteId: string, title: string, dailyDate: string | null) => ({ noteId, title, dailyDate, context: `Line in ${title}`, reference: occurrence });

describe("an event's notes", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });

  it("lists this occurrence's notes, then the rest of the series newest day first", async () => {
    const backlinks = vi.spyOn(mailClient, "noteBacklinks").mockImplementation(async (reference) => reference.id === "standup"
      ? [row("daily:2026-11-12", "2026-11-12", "2026-11-12"), row("daily:2026-11-03", "2026-11-03", "2026-11-03"), row("standup-note", "Standup", null), row("daily:2026-11-10", "2026-11-10", "2026-11-10")]
      : [row("daily:2026-11-12", "2026-11-12", "2026-11-12")]);
    render(<EventNoteBacklinks occurrence={occurrence} onOpenNote={vi.fn()} />);
    const series = await screen.findByRole("region", { name: "Series notes" });
    expect(within(screen.getByRole("region", { name: "Notes" })).getAllByRole("button", { name: /^2026/ }).map((b) => b.textContent?.replace(/Line in .*/, ""))).toEqual(["2026-11-12"]);
    expect(within(series).getAllByRole("button", { name: /^(2026|Standup)/ }).map((b) => b.textContent?.replace(/Line in .*/, ""))).toEqual(["2026-11-10", "2026-11-03", "Standup"]);
    expect(backlinks).toHaveBeenCalledWith({ version: 1, kind: "event", id: "standup", account: "you@example.com", calendar: "work" });
  });

  it("shows no series section for an event that isn't recurring", async () => {
    const backlinks = vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([row("n1", "Retro", null)]);
    render(<EventNoteBacklinks occurrence={{ ...occurrence, series: undefined }} />);
    expect(await screen.findByRole("region", { name: "Notes" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Series notes" })).not.toBeInTheDocument();
    expect(backlinks).toHaveBeenCalledTimes(1);
  });
});

describe("the event note action", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); });
  it("reflects occurrence links and refreshes when a linked note is added or removed", async () => {
    const backlinks = vi.spyOn(mailClient, "noteBacklinks").mockResolvedValue([]);
    const onActivate = vi.fn();
    render(<EventNoteAction occurrence={occurrence} onActivate={onActivate} />);
    fireEvent.click(await screen.findByRole("button", { name: "Add Note" }));
    expect(onActivate).toHaveBeenCalledOnce();
    backlinks.mockResolvedValue([row("existing", "Existing meeting note", null)]);
    await act(async () => { await notesChanged("existing"); });
    fireEvent.click(await screen.findByRole("button", { name: "Open Note" }));
    expect(onActivate).toHaveBeenCalledTimes(2);
    expect(backlinks.mock.calls.every(([reference]) => reference.id === occurrence.id)).toBe(true);
    backlinks.mockResolvedValue([]);
    await act(async () => { await notesChanged("existing"); });
    expect(await screen.findByRole("button", { name: "Add Note" })).toBeInTheDocument();
  });
});
