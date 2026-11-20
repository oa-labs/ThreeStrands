import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";
import { snapshotMessageSelection } from "./messageSelection";
import { todayTarget } from "./noteDrafts";

vi.mock("./messageSelection", () => ({ snapshotMessageSelection: vi.fn(() => null) }));

/** Wiring only: the command, the ⋯ item, and the message each acts on. The dialog and the write have their own tests. */
describe("Add to Note wiring", () => {
  beforeEach(async () => {
    localStorage.clear();
    const detail = await mailClient.getThread("welcome");
    const latest = detail.messages[0]!;
    vi.spyOn(mailClient, "getThread").mockResolvedValue({
      ...detail,
      messages: [
        { ...latest, id: "first", sender: "Ada Lovelace <ada@example.com>", sentAt: "2026-03-03T16:30:00Z", bodyHtml: "<p>First body</p>", bodyText: "First body", unread: true },
        { ...latest, id: "second", sender: "Grace Hopper <grace@example.com>", sentAt: "2026-03-04T16:30:00Z", bodyHtml: "<p>Second body</p>", bodyText: "Second body", unread: true },
      ],
    });
  });
  afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.mocked(snapshotMessageSelection).mockReturnValue(null); localStorage.clear(); });

  const openReader = async () => {
    const view = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    await waitFor(() => expect(view.container.querySelector('[data-message-id="second"]')).toHaveClass("message-active"));
    return view;
  };

  it("W opens the dialog on the message being read, as a link with the option to quote its text", async () => {
    await openReader();
    fireEvent.keyDown(document.body, { key: "w" });
    const dialog = within(await screen.findByRole("dialog", { name: "Add to Note" }));
    expect(dialog.getByRole("region", { name: "Adding" })).toHaveTextContent("A link to the conversation");
    fireEvent.click(dialog.getByRole("checkbox", { name: "Include message text" }));
    expect(dialog.getByRole("region", { name: "Adding" })).toHaveTextContent("From Grace Hopper");
    expect(dialog.getByRole("region", { name: "Adding" })).toHaveTextContent("Second body");
  });

  it("acts on the message that holds the selection, which becomes the quote", async () => {
    await openReader();
    vi.mocked(snapshotMessageSelection).mockReturnValue({ messageId: "first", text: "a selected passage" });
    fireEvent.keyDown(document.body, { key: "w" });
    const adding = within(await screen.findByRole("dialog", { name: "Add to Note" })).getByRole("region", { name: "Adding" });
    expect(adding).toHaveTextContent("a selected passage");
    expect(adding).toHaveTextContent("From Ada Lovelace");
  });

  it("the ⋯ menu's Add to Note… keeps the selection read as the menu began to open", async () => {
    await openReader();
    const snapshot = vi.mocked(snapshotMessageSelection);
    snapshot.mockReturnValue({ messageId: "first", text: "kept passage" });
    const trigger = screen.getByRole("button", { name: "Conversation actions" });
    fireEvent.pointerDown(trigger);
    snapshot.mockReturnValue(null);
    fireEvent.click(trigger, { detail: 1 });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Add to Note…" }));
    expect(screen.queryByRole("menuitem", { name: "Add to Today's Note" })).not.toBeInTheDocument();
    expect(within(await screen.findByRole("dialog", { name: "Add to Note" })).getByRole("region", { name: "Adding" })).toHaveTextContent("kept passage");
  });

  it("adds to today's note from the dialog with an Open Note notice, and Undo takes it back", async () => {
    await openReader();
    vi.mocked(snapshotMessageSelection).mockReturnValue({ messageId: "second", text: "quoted line" });
    fireEvent.keyDown(document.body, { key: "w" });
    const dialog = within(await screen.findByRole("dialog", { name: "Add to Note" }));
    fireEvent.click(await dialog.findByRole("option", { name: /Today's note/ }));
    fireEvent.click(dialog.getByRole("button", { name: "Add to Note" }));
    const notice = (await screen.findByText("Added to today's note")).closest("[role='status']") as HTMLElement;
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add to Note" })).not.toBeInTheDocument());
    expect(await within(notice).findByRole("button", { name: "Open Note" })).toBeInTheDocument();
    const day = todayTarget().date;
    expect((await mailClient.getDailyNote(day))?.body).toMatch(/^> quoted line\n> — \[Welcome to ThreeStrands\]/);
    fireEvent.click(within(notice).getByRole("button", { name: "Undo" }));
    await waitFor(async () => expect((await mailClient.getDailyNote(day))?.body).toBe(""));
  });
});
