import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App, NOTICE_TIMEOUT_MS } from "./App";
import { mailClient } from "./data/client";
import { FOREGROUND_DEBOUNCE_MS, FOREGROUND_IDLE_MS } from "./foregroundRefresh";

const demoThreadIds = ["welcome", "roadmap", "privacy"];

async function archiveSelected() {
  const button = await screen.findByRole("button", { name: "Archive (e)" });
  await act(async () => {
    button.click();
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("archive notice", () => {
  beforeEach(async () => {
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
      await mailClient.mutateThread({ kind: "label", threadId, labelId: "work", value: false });
    }
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("dismisses itself after the notice timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation archived");

    await advance(NOTICE_TIMEOUT_MS - 500);
    expect(screen.getByRole("status")).toBeInTheDocument();

    await advance(500);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("can still be dismissed manually before the timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    const dismiss = await screen.findByRole("button", { name: "Dismiss" });
    await act(async () => {
      dismiss.click();
    });

    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("resolves opaque Gmail label ids with the conversation account's label catalog", async () => {
    const originalLabels = await mailClient.listLabels();
    const listLabels = vi.spyOn(mailClient, "listLabels").mockResolvedValue([
      ...originalLabels,
      { id: "Label_18", name: "Projects", kind: "user" },
    ]);
    await mailClient.mutateThread({
      kind: "label",
      threadId: "welcome",
      labelId: "Label_18",
      value: true,
    });

    try {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to Dispatch" });

      expect(screen.queryByText(/Label_18/i)).not.toBeInTheDocument();
      expect(await screen.findByText("Inbox · Projects")).toBeInTheDocument();
      expect(listLabels).toHaveBeenCalledWith("demo@example.com");
    } finally {
      await mailClient.mutateThread({
        kind: "label",
        threadId: "welcome",
        labelId: "Label_18",
        value: false,
      });
      listLabels.mockRestore();
    }
  });

  it("marks an archived conversation not done with Shift+e", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    const search = screen.getByRole("textbox", { name: "Search mail" });
    fireEvent.change(search, { target: { value: "Welcome" } });
    const includeArchived = await screen.findByRole("button", { name: "Include archived or trashed mail in search" });
    await act(async () => {
      includeArchived.click();
    });
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });

    expect(await screen.findByRole("status")).toHaveTextContent("Conversation marked as not done");
  });

  it("confirms and sends unsubscribe with Cmd+u", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    expect(await screen.findByRole("button", { name: "Unsubscribe (⌘U)" })).toBeVisible();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "u", metaKey: true }));
    });

    const dialog = await screen.findByRole("dialog", { name: "Unsubscribe" });
    expect(dialog).toHaveTextContent("dispatch.example");
    expect(dialog).toHaveTextContent("one-click request");
    await act(async () => {
      screen.getByRole("button", { name: "Send one-click request" }).click();
    });

    expect(await screen.findByRole("status")).toHaveTextContent("Unsubscribe request sent");
    expect(screen.queryByRole("dialog", { name: "Unsubscribe" })).not.toBeInTheDocument();
  });

  it("keeps a conversation unread after pressing u, instead of the auto-read timer reverting it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    // Let the conversation's own auto-read timer (armed on open, since it
    // started unread) run out first so it doesn't interfere with the assertion below.
    await advance(3000);
    await screen.findByRole("button", { name: "Mark unread (u)" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "u" }));
    });
    await screen.findByRole("button", { name: "Mark read (u)" });

    await advance(3000);
    expect(screen.getByRole("button", { name: "Mark read (u)" })).toBeInTheDocument();
  });

  it("undoes an archive and optimistically restores the conversation", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    const undo = await screen.findByRole("button", { name: "Undo" });
    await act(async () => {
      undo.click();
    });

    expect(await screen.findByRole("heading", { name: "Welcome to Dispatch" })).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("undoes the last action with the Superhuman Z shortcut", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    await screen.findByRole("status");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "z" }));
    });

    expect(await screen.findByRole("heading", { name: "Welcome to Dispatch" })).toBeInTheDocument();
  });

  it("undoes adding and removing a label", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    await act(async () => {
      screen.getByRole("button", { name: "Labels (l)" }).click();
    });
    const work = await screen.findByRole("checkbox", { name: "Work" });

    await act(async () => {
      work.click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Work added");
    expect(work).toBeChecked();
    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    await waitFor(() => expect(work).not.toBeChecked());

    await act(async () => {
      work.click();
    });
    await screen.findByText("Work added");
    await act(async () => {
      work.click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Work removed");
    expect(work).not.toBeChecked();
    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    await waitFor(() => expect(work).toBeChecked());
  });

  it("sorts labels alphabetically and supports keyboard navigation", async () => {
    const keyboardLabel = await mailClient.createLabel("Keyboard navigation");
    try {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to Dispatch" });
      await act(async () => {
        screen.getByRole("button", { name: "Labels (l)" }).click();
      });

      const work = await screen.findByRole("checkbox", { name: "Work" });
      const keyboard = await screen.findByRole("checkbox", { name: "Keyboard navigation" });
      expect(screen.getAllByRole("checkbox").map((checkbox) => checkbox.closest("label")?.textContent?.trim()))
        .toEqual(["Keyboard navigation", "Work"]);
      expect(keyboard).toHaveFocus();

      fireEvent.keyDown(keyboard, { key: "ArrowDown" });
      expect(work).toHaveFocus();
      fireEvent.keyDown(work, { key: "ArrowUp" });
      expect(keyboard).toHaveFocus();

      fireEvent.keyDown(keyboard, { key: " ", code: "Space" });
      await waitFor(() => expect(keyboard).toBeChecked());
    } finally {
      await mailClient.deleteLabel(keyboardLabel.id);
    }
  });

  it("keeps a replacement notice on screen for its own full timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    await advance(NOTICE_TIMEOUT_MS - 1000);
    await archiveSelected();

    await advance(1500);
    expect(screen.getByRole("status")).toHaveTextContent("Conversation archived");

    await advance(NOTICE_TIMEOUT_MS);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("clears the pending dismiss timer when the app unmounts", async () => {
    const setTimeout = vi.spyOn(window, "setTimeout");
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const { unmount } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await archiveSelected();
    await screen.findByRole("status");
    const dismissTimer = setTimeout.mock.results
      .filter((_, index) => setTimeout.mock.calls[index][1] === NOTICE_TIMEOUT_MS)
      .map((result) => result.value)
      .at(-1);
    expect(dismissTimer).toBeDefined();

    unmount();
    expect(clearTimeout).toHaveBeenCalledWith(dismissTimer);
    setTimeout.mockRestore();
    clearTimeout.mockRestore();
  });
});

describe("trash and batch actions", () => {
  beforeEach(async () => {
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "trash", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
      await mailClient.mutateThread({ kind: "label", threadId, labelId: "work", value: false });
    }
  });

  afterEach(cleanup);

  it("moves the open conversation to trash and can undo it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await act(async () => {
      screen.getByRole("button", { name: "Trash (#)" }).click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation moved to trash");
    expect(screen.queryByRole("heading", { name: "Welcome to Dispatch" })).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    expect(await screen.findByRole("heading", { name: "Welcome to Dispatch" })).toBeInTheDocument();
  });

  it("marks the open conversation as spam with ! and can undo it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "!",
        code: "Digit1",
        shiftKey: true,
      }));
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation marked as spam");
    expect(screen.queryByRole("heading", { name: "Welcome to Dispatch" })).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    expect(await screen.findByRole("heading", { name: "Welcome to Dispatch" })).toBeInTheDocument();
  });

  it("archives every conversation checked for batch actions", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "x" }));
    });
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "j" }));
    });
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "x" }));
    });
    expect(await screen.findByText("2 selected")).toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "Archive" }).click();
    });

    expect(await screen.findByRole("status")).toHaveTextContent("Archived 2 conversations");
    expect(screen.queryByText("2 selected")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Welcome to Dispatch" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Phase 1: read and triage" })).not.toBeInTheDocument();
  });
});

describe("Escape dismissal", () => {
  beforeEach(async () => {
    localStorage.removeItem("dispatch.demoCorrespondence");
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
    }
  });

  afterEach(cleanup);

  it("closes the composer when focus is in a field", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    fireEvent.click(screen.getByRole("button", { name: "New message (c)" }));

    const recipient = await screen.findByRole("textbox", { name: "To" });
    recipient.focus();
    fireEvent.keyDown(recipient, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "New message" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("region", { name: "Conversation" })).toBeInTheDocument();
  });

  it("switches to the inline Drafts view and back to the inbox without losing state", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    fireEvent.click(screen.getByRole("button", { name: /Drafts \(0\)/ }));

    expect(await screen.findByRole("heading", { name: "0 drafts" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Welcome to Dispatch" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Inbox (g then i)" }));
    expect(await screen.findByRole("heading", { name: "Welcome to Dispatch" })).toBeInTheDocument();
  });

  it("closes only the topmost popup when overlays are stacked", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    fireEvent.click(screen.getByRole("button", { name: "New message (c)" }));
    const composer = await screen.findByRole("dialog", { name: "New message" });
    fireEvent.click(screen.getByRole("button", { name: "Command palette" }));

    const filter = await screen.findByRole("textbox", { name: "Filter commands" });
    fireEvent.keyDown(filter, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument(),
    );
    expect(composer).toBeInTheDocument();
  });
});

describe("foreground mail refresh", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
  });

  it("checks for new mail after the window has been in the background", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const sync = vi.spyOn(mailClient, "sync");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    sync.mockClear();

    Object.defineProperty(document, "visibilityState", { configurable: true, value: "hidden" });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_IDLE_MS);
    });
    Object.defineProperty(document, "visibilityState", { configurable: true, value: "visible" });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_DEBOUNCE_MS);
    });

    expect(sync).toHaveBeenCalled();
  });

  it("does not sync on a brief focus flicker", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const sync = vi.spyOn(mailClient, "sync");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    sync.mockClear();

    await act(async () => {
      window.dispatchEvent(new Event("blur"));
      await vi.advanceTimersByTimeAsync(200);
      window.dispatchEvent(new Event("focus"));
      await vi.advanceTimersByTimeAsync(FOREGROUND_DEBOUNCE_MS);
    });

    expect(sync).not.toHaveBeenCalled();
  });
});
