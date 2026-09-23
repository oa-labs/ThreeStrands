import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountSwitcher, App, NOTICE_TIMEOUT_MS } from "./App";
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
    localStorage.removeItem("threestrands.demoCorrespondence");
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
      await mailClient.mutateThread({ kind: "label", threadId, labelId: "work", value: false });
    }
    localStorage.removeItem("threestrands.settings.autoReadDelaySeconds");
    localStorage.removeItem("threestrands.settings.labelUsageByAccount");
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("dismisses itself after the notice timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await archiveSelected();
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation archived");

    await advance(NOTICE_TIMEOUT_MS - 500);
    expect(screen.getByRole("status")).toBeInTheDocument();

    await advance(500);
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
  });

  it("can still be dismissed manually before the timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

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
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      expect(screen.queryByText(/Label_18/i)).not.toBeInTheDocument();
      expect(await screen.findByText("Inbox", { selector: ".eyebrow" })).toBeInTheDocument();
      expect(await screen.findByText("Projects", { selector: ".user-label-badge" })).toBeInTheDocument();
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

  it("toggles only the prior message card whose header was clicked", async () => {
    const originalDetail = await mailClient.getThread("welcome");
    const latest = originalDetail.messages[0]!;
    const getThread = vi.spyOn(mailClient, "getThread").mockResolvedValue({
      ...originalDetail,
      messages: [
        {
          ...latest,
          id: "welcome-first",
          sentAt: "2026-03-03T16:30:00Z",
          bodyHtml: "<p>First message body</p>",
          bodyText: "First message snippet",
          unread: false,
        },
        {
          ...latest,
          id: "welcome-second",
          sentAt: "2026-03-04T16:30:00Z",
          bodyHtml: "<p>Second message body</p>",
          bodyText: "Second message snippet",
          unread: false,
        },
        { ...latest, unread: false },
      ],
    });

    try {
      const { container } = render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      expect(container.querySelectorAll("article.message-card")).toHaveLength(3);
      expect(container.querySelectorAll("button.message-card-toggle[aria-expanded='false']")).toHaveLength(2);
      expect(screen.getAllByTestId("message-body")).toHaveLength(1);

      const firstArticle = container.querySelector<HTMLElement>('[data-message-id="welcome-first"]')!;
      const secondArticle = container.querySelector<HTMLElement>('[data-message-id="welcome-second"]')!;
      const latestArticle = container.querySelector<HTMLElement>(`[data-message-id="${latest.id}"]`)!;
      expect(latestArticle).toHaveClass("message-card-expanded", "message-active");

      let firstHeader = within(firstArticle).getByRole("button", { name: /First message snippet/ });
      expect(firstHeader).toHaveAttribute("aria-expanded", "false");
      expect(firstHeader).toHaveAttribute("aria-controls", "message-body-0");
      firstHeader.focus();
      fireEvent.keyDown(firstHeader, { key: "Enter" });

      // Expanding swaps the condensed snippet header for the full sender/recipient header.
      firstHeader = within(firstArticle).getByRole("button", { name: /Collapse message from/ });
      expect(firstHeader).toHaveClass("message-expanded-toggle");
      expect(firstArticle.querySelector("header")).toHaveClass("message-expanded-header");
      expect(firstHeader).toHaveAttribute("aria-expanded", "true");
      expect(firstArticle).toHaveClass("message-card-expanded", "message-active");
      expect(latestArticle).not.toHaveClass("message-active");
      expect(firstHeader).toHaveFocus();
      expect(screen.getAllByTestId("message-body")).toHaveLength(2);
      expect(within(secondArticle).getByRole("button", { name: /Second message snippet/ })).toBeInTheDocument();

      fireEvent.keyDown(firstHeader, { key: "Enter" });

      firstHeader = within(firstArticle).getByRole("button", { name: /First message snippet/ });
      expect(firstHeader).toHaveAttribute("aria-expanded", "false");
      expect(firstHeader).toHaveFocus();
      expect(screen.getAllByTestId("message-body")).toHaveLength(1);

      const secondHeader = within(secondArticle).getByRole("button", { name: /Second message snippet/ });
      const latestHeader = within(latestArticle).getByRole("button", { name: /Collapse message from/ });
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight" }));
      });
      expect(secondHeader).toHaveFocus();
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "n" }));
      });
      expect(latestHeader).toHaveFocus();
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "p" }));
      });
      expect(secondHeader).toHaveFocus();
      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft" }));
      });
      expect(firstHeader).toHaveFocus();

      fireEvent.click(latestHeader);
      expect(latestArticle).toHaveClass("message-card-collapsed", "message-active");
      expect(within(latestArticle).getByRole("button")).toHaveAttribute("aria-expanded", "false");
    } finally {
      getThread.mockRestore();
    }
  });

  it("places message actions to the left of the received time", async () => {
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    const senderRow = container.querySelector<HTMLElement>(".message-card-expanded .message-sender-row")!;
    const actions = senderRow.querySelector<HTMLElement>(".message-header-actions")!;
    const receivedTime = senderRow.querySelector("time")!;

    expect(Array.from(senderRow.children).indexOf(actions)).toBeLessThan(
      Array.from(senderRow.children).indexOf(receivedTime),
    );
    expect(within(actions).getAllByRole("button").map((button) => button.getAttribute("aria-label")))
      .toEqual(["Reply", "Reply All", "Forward"]);
  });

  it("keeps multiple unread messages expanded when auto-read marks the conversation read", async () => {
    await mailClient.mutateThread({ kind: "read", threadId: "welcome", value: false });
    const originalDetail = await mailClient.getThread("welcome");
    const latest = originalDetail.messages[0]!;
    const getThread = vi.spyOn(mailClient, "getThread").mockResolvedValue({
      ...originalDetail,
      messages: [
        {
          ...latest,
          id: "welcome-first-unread",
          sentAt: "2026-03-03T16:30:00Z",
          bodyHtml: "<p>First unread message body</p>",
          bodyText: "First unread message snippet",
          unread: true,
        },
        {
          ...latest,
          id: "welcome-second-unread",
          sentAt: "2026-03-04T16:30:00Z",
          bodyHtml: "<p>Second unread message body</p>",
          bodyText: "Second unread message snippet",
          unread: true,
        },
        { ...latest, unread: true },
      ],
    });

    try {
      const { container } = render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      expect(container.querySelectorAll("button.message-card-toggle[aria-expanded='false']")).toHaveLength(0);
      expect(container.querySelectorAll("[aria-expanded='true']")).toHaveLength(3);
      expect(screen.getAllByTestId("message-body")).toHaveLength(3);

      await advance(3000);
      await screen.findByRole("button", { name: "Mark Unread (u)" });

      expect(container.querySelectorAll("button.message-card-toggle[aria-expanded='false']")).toHaveLength(0);
      expect(container.querySelectorAll("[aria-expanded='true']")).toHaveLength(3);
      expect(screen.getAllByTestId("message-body")).toHaveLength(3);
    } finally {
      getThread.mockRestore();
    }
  });

  it("marks an archived conversation not done with Shift+e", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await archiveSelected();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "/" }));
    });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "Welcome" } });
    const includeArchived = await screen.findByRole("button", { name: "Include archived or trashed mail in search" });
    await act(async () => {
      includeArchived.click();
    });
    expect(includeArchived).toHaveTextContent("Archived + Trash");
    expect(includeArchived).toHaveAttribute("aria-pressed", "true");
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });

    expect(await screen.findByRole("status")).toHaveTextContent("Conversation marked as not done");
  });

  it("confirms and sends unsubscribe with Cmd+u", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    expect(await screen.findByRole("button", { name: "Unsubscribe (⌘U)" })).toBeVisible();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "u", metaKey: true }));
    });

    const dialog = await screen.findByRole("dialog", { name: "Unsubscribe" });
    expect(dialog).toHaveTextContent("threestrands.example");
    expect(dialog).toHaveTextContent("One-Click Request");
    await act(async () => {
      screen.getByRole("button", { name: "Send One-Click Request" }).click();
    });

    expect(await screen.findByRole("status")).toHaveTextContent("Unsubscribe request sent");
    expect(screen.queryByRole("dialog", { name: "Unsubscribe" })).not.toBeInTheDocument();
  });

  it("keeps a conversation unread after pressing u, instead of the auto-read timer reverting it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    // Let the conversation's own auto-read timer (armed on open, since it
    // started unread) run out first so it doesn't interfere with the assertion below.
    await advance(3000);
    await screen.findByRole("button", { name: "Mark Unread (u)" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "u" }));
    });
    await screen.findByRole("button", { name: "Mark Read (u)" });

    await advance(3000);
    expect(screen.getByRole("button", { name: "Mark Read (u)" })).toBeInTheDocument();
  });

  it("reschedules auto-read when its delay changes without reverting an explicit unread action", async () => {
    await mailClient.mutateThread({ kind: "read", threadId: "welcome", value: false });
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "60");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    expect(screen.getByRole("button", { name: "Mark Read (u)" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    fireEvent.click(within(settings).getByRole("button", { name: "Reading" }));
    fireEvent.change(within(settings).getByRole("spinbutton", { name: "Auto-Read Delay" }), {
      target: { value: "1" },
    });
    fireEvent.keyDown(window, { key: "Escape" });

    await advance(1000);
    await screen.findByRole("button", { name: "Mark Unread (u)" });

    fireEvent.click(screen.getByRole("button", { name: "Mark Unread (u)" }));
    await screen.findByRole("button", { name: "Mark Read (u)" });
    await advance(2000);
    expect(screen.getByRole("button", { name: "Mark Read (u)" })).toBeInTheDocument();
  });

  it("does not scroll away from a reply when the delayed auto-read update runs", async () => {
    await mailClient.mutateThread({ kind: "read", threadId: "welcome", value: false });
    const scrollIntoView = vi.fn();
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;

    try {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
      scrollIntoView.mockClear();

      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "r" }));
      });
      await screen.findByRole("dialog", { name: "Reply Message" });

      await advance(3000);
      await screen.findByRole("button", { name: "Mark Unread (u)" });
      expect(scrollIntoView).not.toHaveBeenCalled();
    } finally {
      if (originalScrollIntoView) HTMLElement.prototype.scrollIntoView = originalScrollIntoView;
      else delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
    }
  });

  it("undoes an archive and optimistically restores the conversation", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await archiveSelected();
    const undo = await screen.findByRole("button", { name: "Undo" });
    await act(async () => {
      undo.click();
    });

    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("undoes the last action with the Superhuman Z shortcut", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await archiveSelected();
    await screen.findByRole("status");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "z" }));
    });

    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
  });

  it("undoes adding and removing a label", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await act(async () => {
      screen.getByRole("button", { name: "Labels (l)" }).click();
    });
    await act(async () => {
      (await screen.findByRole("option", { name: "Work" })).click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Work added");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add label" })).not.toBeInTheDocument());

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });

    await act(async () => {
      screen.getByRole("button", { name: "Labels (l)" }).click();
    });
    await screen.findByRole("option", { name: "Work" });

    await act(async () => {
      screen.getByRole("option", { name: "Work" }).click();
    });
    await screen.findByText("Work added");

    await act(async () => {
      screen.getByRole("button", { name: "Labels (l)" }).click();
    });
    await act(async () => {
      (await screen.findByRole("option", { name: "Work, added" })).click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Work removed");

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });

    await act(async () => {
      screen.getByRole("button", { name: "Labels (l)" }).click();
    });
    await screen.findByRole("option", { name: "Work, added" });
  });

  it("orders labels alphabetically and supports search plus keyboard navigation", async () => {
    const keyboardLabel = await mailClient.createLabel("Keyboard navigation");
    try {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      await act(async () => {
        screen.getByRole("button", { name: "Labels (l)" }).click();
      });

      const dialog = screen.getByRole("dialog", { name: "Manage Labels" });
      const input = await within(dialog).findByRole("combobox", { name: "Find or Create a Label" });
      expect(input).toHaveFocus();
      expect(within(dialog).getAllByRole("option").map((option) => option.textContent))
        .toEqual(["Keyboard navigation", "Work"]);
      expect(within(dialog).getAllByRole("option")[0]).toHaveClass("highlighted");

      fireEvent.keyDown(input, { key: "ArrowDown" });
      expect(within(dialog).getAllByRole("option")[1]).toHaveClass("highlighted");
      fireEvent.keyDown(input, { key: "ArrowUp" });
      expect(within(dialog).getAllByRole("option")[0]).toHaveClass("highlighted");

      fireEvent.change(input, { target: { value: "key" } });
      expect(within(dialog).getAllByRole("option").map((option) => option.textContent))
        .toEqual(["Keyboard navigation", 'Create label "key"']);

      await act(async () => {
        fireEvent.keyDown(input, { key: "Enter" });
      });
      expect(await screen.findByRole("status")).toHaveTextContent("Keyboard navigation added");
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add label" })).not.toBeInTheDocument());
    } finally {
      await mailClient.deleteLabel(keyboardLabel.id);
    }
  });

  it("creates and applies a new label from the search box", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    try {
      await act(async () => {
        screen.getByRole("button", { name: "Labels (l)" }).click();
      });

      const input = await screen.findByRole("combobox", { name: "Find or Create a Label" });
      fireEvent.change(input, { target: { value: "Project X" } });
      const createRow = await screen.findByRole("option", { name: 'Create label "Project X"' });
      expect(createRow).toHaveClass("highlighted");

      await act(async () => {
        fireEvent.keyDown(input, { key: "Enter" });
      });
      expect(await screen.findByRole("status")).toHaveTextContent("Project X added");
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add label" })).not.toBeInTheDocument());

      await act(async () => {
        screen.getByRole("button", { name: "Labels (l)" }).click();
      });
      const created = await screen.findByRole("option", { name: "Project X, added" });
      await act(async () => {
        created.click();
      });
      expect(await screen.findByRole("status")).toHaveTextContent("Project X removed");
    } finally {
      const projectX = (await mailClient.listLabels()).find((label) => label.name === "Project X");
      if (projectX) await mailClient.deleteLabel(projectX.id);
    }
  });

  it("offers a freshly created label, sorted alphabetically, as a split inbox match", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    try {
      await act(async () => {
        screen.getByRole("button", { name: "Labels (l)" }).click();
      });
      const input = await screen.findByRole("combobox", { name: "Find or Create a Label" });
      fireEvent.change(input, { target: { value: "Aardvark" } });
      await act(async () => {
        fireEvent.keyDown(input, { key: "Enter" });
      });
      await screen.findByRole("status");
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add label" })).not.toBeInTheDocument());

      fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
      const settings = await screen.findByRole("dialog", { name: "Settings" });
      fireEvent.click(within(settings).getByRole("button", { name: "Split Inboxes" }));
      fireEvent.change(within(settings).getByRole("combobox", { name: "Match By" }), {
        target: { value: "label" },
      });

      const labelSelect = within(settings).getByRole("combobox", { name: "Label" });
      expect(within(labelSelect).getAllByRole("option").map((option) => option.textContent))
        .toEqual(["Choose a label", "Aardvark", "Work"]);
    } finally {
      const aardvark = (await mailClient.listLabels()).find((label) => label.name === "Aardvark");
      if (aardvark) await mailClient.deleteLabel(aardvark.id);
    }
  });

  it("keeps a replacement notice on screen for its own full timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

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
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

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

describe("keyboard-first task and action workspaces", () => {
  beforeEach(async () => {
    localStorage.removeItem("threestrands.demoCorrespondence");
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
    }
  });

  afterEach(cleanup);

  it("opens task entry as a modal without changing the right workspace", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Archive (e)" });

    fireEvent.keyDown(window, { key: "d" });
    const dialog = await screen.findByRole("dialog", { name: "Add Task" });
    expect(within(dialog).getByRole("textbox", { name: "Task" })).toHaveFocus();
    expect(screen.queryByRole("complementary", { name: "Actions" })).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Add Task" })).not.toBeInTheDocument();
  });

  it("keeps Actions read-only so letter shortcuts can switch workspaces", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Archive (e)" });

    fireEvent.keyDown(window, { key: "A", shiftKey: true });
    const actions = screen.getByRole("complementary", { name: "Actions" });
    expect(within(actions).queryByRole("textbox")).not.toBeInTheDocument();
    expect(within(actions).queryByRole("combobox")).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "3" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Inbox" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Conversation" })).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "Actions" })).not.toBeInTheDocument();
  });

  it("switches directly and cyclically between mail and task views with number keys", async () => {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    fireEvent.keyDown(window, { key: "3" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "d" });
    const taskDialog = await screen.findByRole("dialog", { name: "Add Task" });
    expect(within(taskDialog).getByRole("textbox", { name: "Task" })).toHaveValue("");
    expect(within(taskDialog).queryByText(/^From:/)).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "0" });
    expect(screen.getByRole("region", { name: "Inbox" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "0" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "1" });
    expect(screen.getByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
  });

  it("opens the week calendar view with 2 and returns to mail with 1", async () => {
    const { container } = render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    fireEvent.keyDown(window, { key: "2" });
    const week = await screen.findByRole("region", { name: "Calendar week" });
    expect(container.querySelector("main")).toHaveClass("week-open");
    expect(within(week).getByRole("region", { name: "Month picker" })).toBeInTheDocument();
    expect(within(week).getByRole("region", { name: "Calendars" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Inbox" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Conversation" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "1" });
    expect(await screen.findByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Calendar week" })).not.toBeInTheDocument();
  });

  it("keeps the weekly calendar out of the sidebar and retains today's schedule", async () => {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    expect(screen.queryByRole("button", { name: "Calendar (2)" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Today’s Schedule (T)" })).toBeInTheDocument();
  });

  it("replaces the mail viewport and manages tasks through the focused keyboard commands", async () => {
    const tasks = [
      {
        id: "task-welcome", accountId: "demo@example.com", threadId: "welcome", sourceMessageId: null,
        subjectSnapshot: "Welcome to ThreeStrands", title: "Read the welcome guide", notes: null,
        kind: "action" as const, dueKind: "none" as const, dueValue: null, timeZone: null,
        repeatIntervalDays: null, status: "open" as const, completionSource: null, evidenceText: null,
        waitAfter: null, createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null,
      },
      {
        id: "task-roadmap", accountId: "demo@example.com", threadId: "roadmap", sourceMessageId: null,
        subjectSnapshot: "Phase 1: read and triage", title: "Review the roadmap", notes: null,
        kind: "action" as const, dueKind: "none" as const, dueValue: null, timeZone: null,
        repeatIntervalDays: null, status: "open" as const, completionSource: null, evidenceText: null,
        waitAfter: null, createdAt: "2026-09-19T11:00:00Z", updatedAt: "2026-09-19T11:00:00Z", completedAt: null,
      },
    ];
    const listTasks = vi.spyOn(mailClient, "listTasks").mockResolvedValue(tasks);
    const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (id) => ({
      ...tasks.find((task) => task.id === id)!, status: "completed", completionSource: "user",
    }));
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => ({
      ...tasks.find((task) => task.id === request.id)!, ...request,
    }));

    try {
      const { container } = render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      fireEvent.keyDown(window, { key: "3" });
      const workspace = await screen.findByRole("region", { name: "Tasks" });
      expect(container.querySelector("main")).toHaveClass("tasks-open");
      await waitFor(() => expect(workspace.querySelector("#task-task-welcome")).toHaveAttribute("aria-current", "true"));

      const calendar = await screen.findByRole("complementary", { name: "Calendar schedule" });
      expect(calendar).toBeInTheDocument();
      expect(within(calendar).queryByRole("button", { name: "Close calendar" })).not.toBeInTheDocument();

      fireEvent.keyDown(window, { key: "ArrowDown" });
      await waitFor(() => expect(workspace.querySelector("#task-task-roadmap")).toHaveAttribute("aria-current", "true"));
      expect(within(screen.getByRole("region", { name: "Task details" })).getByRole("heading", { name: "Review the roadmap" })).toBeInTheDocument();

      fireEvent.keyDown(window, { key: "e" });
      await waitFor(() => expect(setStatus).toHaveBeenCalledWith("task-roadmap", "completed"));

      fireEvent.keyDown(window, { key: "Enter" });
      const dialog = await screen.findByRole("dialog", { name: "Edit Task" });
      fireEvent.change(within(dialog).getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
      fireEvent.change(within(dialog).getByLabelText("Due Date"), { target: { value: "2026-09-30" } });
      fireEvent.click(within(dialog).getByRole("button", { name: "Save Task" }));
      await waitFor(() => expect(updateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "task-roadmap", dueKind: "date", dueValue: "2026-09-30" })));

      fireEvent.keyDown(window, { key: "o" });
      await screen.findByRole("heading", { name: "Phase 1: read and triage" });
      expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
    } finally {
      listTasks.mockRestore();
      setStatus.mockRestore();
      updateTask.mockRestore();
    }
  });
});

describe("trash and batch actions", () => {
  beforeEach(async () => {
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "trash", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
      await mailClient.mutateThread({ kind: "star", threadId, value: false });
      await mailClient.mutateThread({ kind: "label", threadId, labelId: "work", value: false });
    }
  });

  afterEach(cleanup);

  it("moves the open conversation to trash and can undo it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await act(async () => {
      screen.getByRole("button", { name: "Trash (#)" }).click();
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation moved to trash");
    expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
  });

  it("marks the open conversation as spam with ! and can undo it", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", {
        key: "!",
        code: "Digit1",
        shiftKey: true,
      }));
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Conversation marked as spam");
    expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "Undo" }).click();
    });
    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
  });

  it("archives every conversation checked for batch actions", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

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
    expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Phase 1: read and triage" })).not.toBeInTheDocument();
  });

  it("shows one state-aware star action and hover help for every batch action", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "x" }));
    });

    const toolbar = screen.getByRole("toolbar", { name: "Batch actions" });
    expect(within(toolbar).getByText("1 selected")).toBeInTheDocument();
    expect(within(toolbar).getAllByRole("button", { name: /star/i })).toHaveLength(1);
    expect(within(toolbar).getByRole("button", { name: "Star" })).toBeInTheDocument();

    for (const label of ["Archive", "Trash", "Mark spam", "Mark read", "Mark unread", "Star", "Labels", "Clear selection"]) {
      expect(within(toolbar).getByRole("tooltip", { name: label })).toBeInTheDocument();
    }

    await act(async () => {
      within(toolbar).getByRole("button", { name: "Star" }).click();
    });
    const starredToolbar = screen.getByRole("toolbar", { name: "Batch actions" });
    expect(within(starredToolbar).getByText("1 selected")).toBeInTheDocument();
    expect(within(starredToolbar).getByRole("button", { name: "Unstar" })).toBeInTheDocument();
    expect(within(starredToolbar).getAllByRole("button", { name: /star/i })).toHaveLength(1);
  });
});

describe("Escape dismissal", () => {
  beforeEach(async () => {
    localStorage.removeItem("threestrands.demoCorrespondence");
    for (const threadId of demoThreadIds) {
      await mailClient.mutateThread({ kind: "archive", threadId, value: false });
      await mailClient.mutateThread({ kind: "spam", threadId, value: false });
    }
  });

  afterEach(cleanup);

  it("closes the composer when focus is in a field", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "New Message (c)" }));

    const recipient = await screen.findByRole("textbox", { name: "To" });
    recipient.focus();
    fireEvent.keyDown(recipient, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "New Message" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("region", { name: "Conversation" })).toBeInTheDocument();
  });

  it("switches to the inline Drafts view and back to the inbox without losing state", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: /Drafts \(0\)/ }));

    expect(await screen.findByRole("heading", { name: "0 drafts" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Inbox (g then i)" }));
    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
  });

  it("closes only the topmost popup when overlays are stacked", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "New Message (c)" }));
    const composer = await screen.findByRole("dialog", { name: "New Message" });
    fireEvent.click(screen.getByRole("button", { name: "Command Palette" }));

    const filter = await screen.findByRole("textbox", { name: "Filter Commands" });
    fireEvent.keyDown(filter, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Command Palette" })).not.toBeInTheDocument(),
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
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
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
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
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

describe("account selection persistence", () => {
  afterEach(() => {
    cleanup();
    localStorage.removeItem("threestrands.settings.selectedAccountId");
    vi.restoreAllMocks();
  });

  it("restores both a selected account and All accounts after a restart", async () => {
    const [primary] = await mailClient.listAccounts();
    vi.spyOn(mailClient, "listAccounts").mockResolvedValue([
      primary!,
      {
        ...primary!,
        email: "work@example.com",
        displayName: "Work",
        color: "#34A853",
        sortOrder: 1,
      },
    ]);

    const firstRun = render(<App />);
    await act(async () => {});
    const workAccount = screen.getByRole("radio", { name: "Work" });
    fireEvent.click(workAccount);
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("work@example.com");
    firstRun.unmount();

    const secondRun = render(<App />);
    await act(async () => {});
    expect(screen.getByRole("radio", { name: "Work" })).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("radio", { name: "All Accounts" }));
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("all");
    secondRun.unmount();

    render(<App />);
    await act(async () => {});
    expect(screen.getByRole("radio", { name: "All Accounts" })).toHaveAttribute("aria-checked", "true");
  });

  it("shows unread inbox totals on each account and the combined account icon", async () => {
    const primary = {
      email: "demo@example.com",
      displayName: null,
      color: "#4285F4",
      status: "connected" as const,
      provider: "gmail" as const,
      sortOrder: 0,
      connectedAt: "2026-03-04T00:00:00Z",
      lastSyncedAt: null,
    };
    render(
      <AccountSwitcher
        accounts={[
          primary,
          {
        ...primary,
        email: "work@example.com",
        displayName: "Work",
        color: "#34A853",
        sortOrder: 1,
          },
        ]}
        unreadCounts={{ [primary.email]: 3, "work@example.com": 120 }}
        activeAccountId={null}
        onSwitch={() => {}}
        onShowAll={() => {}}
        onReorder={() => {}}
      />,
    );

    expect(screen.getByRole("radio", { name: "All accounts, 123 unread" })).toHaveTextContent("99+");
    expect(screen.getByRole("radio", { name: `${primary.email}, 3 unread` })).toHaveTextContent("3");
    expect(screen.getByRole("radio", { name: "Work, 120 unread" })).toHaveTextContent("99+");
  });
});
