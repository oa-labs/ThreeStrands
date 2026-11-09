import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import type { WorkspaceFile } from "./domain";

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
