import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AccountSwitcher, App, NOTICE_TIMEOUT_MS } from "./App";
import { mailClient } from "./data/client";
import type { ThreadTask } from "./domain";
import { FOREGROUND_DEBOUNCE_MS, FOREGROUND_IDLE_MS } from "./foregroundRefresh";
import { SEARCH_DEBOUNCE_MS } from "./SearchField";

const demoThreadIds = ["welcome", "roadmap", "privacy"];

function selectFolder(name: string) {
  fireEvent.click(screen.getByRole("button", { name: /Choose folder, current folder/ }));
  fireEvent.click(within(screen.getByRole("group", { name: "Folders" })).getByRole("button", { name }));
}

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

async function resetDemoThreads({ labels = false }: { labels?: boolean } = {}) {
  localStorage.removeItem("threestrands.demoCorrespondence");
  for (const threadId of demoThreadIds) {
    await mailClient.mutateThread({ kind: "archive", threadId, value: false });
    await mailClient.mutateThread({ kind: "spam", threadId, value: false });
    if (labels) await mailClient.mutateThread({ kind: "label", threadId, labelId: "work", value: false });
  }
}

/** Resets demo threads and reading settings, and runs each test on advanceable fake timers. */
function useConversationFixture() {
  beforeEach(async () => {
    await resetDemoThreads({ labels: true });
    localStorage.removeItem("threestrands.settings.autoReadDelaySeconds");
    localStorage.removeItem("threestrands.settings.labelUsageByAccount");
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });

  afterEach(() => {
    cleanup();
    // Restore spies (including any on timer functions) while the fake clock is
    // still installed, so a later restoreAllMocks cannot reinstate a fake timer.
    vi.restoreAllMocks();
    vi.useRealTimers();
  });
}

describe("archive notice", () => {
  useConversationFixture();

  it("dismisses itself after the notice timeout", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    expect(document.querySelector(".reader-account-scope")).toHaveTextContent("demo@example.com");

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

describe("undo", () => {
  useConversationFixture();

  it.each([false, true])("unarchives the last email with Shift+E (notice expired: %s)", async (expireNotice) => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

    await archiveSelected();
    await screen.findByRole("status");
    expect(screen.queryByRole("option", { name: /Welcome to ThreeStrands/ })).not.toBeInTheDocument();
    if (expireNotice) {
      await advance(NOTICE_TIMEOUT_MS);
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    }
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });

    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
    expect((await mailClient.listThreads()).find((thread) => thread.id === "welcome")?.archived).toBe(false);
    // Consuming the archive undo also consumes the generic Z undo.
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "z" }));
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("unarchives the last email with Shift+E when the inbox is empty", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    for (let index = 0; index < demoThreadIds.length; index++) {
      await archiveSelected();
      await screen.findByRole("status");
    }
    expect(screen.queryAllByRole("option")).toHaveLength(0);

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });

    expect(await screen.findByRole("heading", { name: "Your inbox stays local" })).toBeInTheDocument();
    expect(screen.getAllByRole("option")).toHaveLength(1);
  });

  it("does not use Shift+E to undo a different action that replaces the archive undo", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    await archiveSelected();
    await screen.findByRole("status");
    const mutateThreads = vi.spyOn(mailClient, "mutateThreads");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "s" }));
    });
    await screen.findByRole("status");
    mutateThreads.mockClear();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });

    expect(mutateThreads).not.toHaveBeenCalled();
    expect((await mailClient.listAllMail()).find((thread) => thread.id === "welcome")?.archived).toBe(true);
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

  it("undoes an archive made in another folder without repainting the folder now on screen", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    await archiveSelected();
    await screen.findByRole("status");

    selectFolder("Trash");
    await waitFor(() => expect(screen.queryAllByRole("option")).toHaveLength(0));
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "z" }));
    });
    await advance(50);

    // The Inbox rows the archive captured stay out of Trash.
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.queryByRole("option", { name: /Welcome to ThreeStrands/ })).not.toBeInTheDocument();
    // The archive itself was still undone.
    expect((await mailClient.listThreads()).find((thread) => thread.id === "welcome")?.archived).toBe(false);
    selectFolder("Inbox");
    expect(await screen.findByRole("option", { name: /Welcome to ThreeStrands/ })).toBeInTheDocument();
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
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Manage Labels" })).not.toBeInTheDocument());

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
});

describe("conversation labels", () => {
  useConversationFixture();

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
      expect(await screen.findByText("Inbox", { selector: "span.eyebrow" })).toBeInTheDocument();
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
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Manage Labels" })).not.toBeInTheDocument());
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
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Manage Labels" })).not.toBeInTheDocument());

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
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Manage Labels" })).not.toBeInTheDocument());

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
});

describe("message cards", () => {
  useConversationFixture();

  for (const html of [false, true]) {
    it.each([
      ["r", "Reply", "reply"],
      ["a", "Reply All", "replyAll"],
      ["f", "Forward", "forward"],
      [null, "Reply", "reply"],
      [null, "Reply All", "replyAll"],
      [null, "Forward", "forward"],
    ] as const)(`quotes selected ${html ? "HTML" : "plain"} message text using %s / %s`, async (key, label, mode) => {
      const original = await mailClient.getThread("welcome");
      const selected = "Selected <img> & passage";
      vi.spyOn(mailClient, "getThread").mockResolvedValue({
        ...original,
        messages: [{ ...original.messages[0], bodyHtml: html ? "<p>Selected &lt;img&gt; &amp; passage</p>" : "", bodyText: selected }],
      });
      const createDraft = vi.spyOn(mailClient, "createDraft");
      const saveDraft = vi.spyOn(mailClient, "saveDraft");
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const body = screen.getByTestId("message-body");
      const frame = html ? body as HTMLIFrameElement : null;
      const doc = frame?.contentDocument ?? document;
      // jsdom does not load srcdoc; install the selected text in its frame document.
      if (frame) doc.body.textContent = selected;
      const range = doc.createRange();
      range.selectNodeContents(frame ? doc.body : body);
      const selection = frame ? frame.contentWindow!.getSelection()! : window.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
      if (frame) frame.focus();

      if (key) fireEvent.keyDown(window, { key });
      else {
        const button = screen.getByRole("button", { name: label });
        expect(fireEvent.mouseDown(button, { button: 0 })).toBe(false);
        fireEvent.click(button);
      }

      const composer = await screen.findByRole("dialog", { name: mode === "forward" ? "Forward Message" : "Reply Message" });
      expect(createDraft).toHaveBeenCalledWith(mode, original.messages[0].id, original.thread.accountId);
      await waitFor(() => expect(saveDraft).toHaveBeenCalled());
      const saved = saveDraft.mock.calls[0][0];
      if (mode === "forward") {
        expect(saved.forwardedContent?.text).toContain(`> ${selected}`);
        expect(saved.forwardedContent?.html).toContain('blockquote type="cite"');
        expect(within(composer).getByRole("region", { name: "Forwarded message" })).toBeInTheDocument();
      } else {
        expect(within(composer).getByRole("textbox", { name: "Quoted Text" })).toBeVisible();
        expect(within(composer).getByRole("textbox", { name: "Quoted Text" }).querySelector("blockquote")).toHaveTextContent(selected);
        expect(within(composer).getByRole("textbox", { name: "Message Body" })).toHaveFocus();
        expect(saved.body).toContain(`> ${selected}`);
      }
      expect(JSON.stringify(saved)).not.toContain("A keyboard-first inbox");
      selection.removeAllRanges();
    });
  }

  it("expands, activates, and scrolls to a message picked from the context panel's thread outline", async () => {
    const originalDetail = await mailClient.getThread("welcome");
    const latest = originalDetail.messages[0]!;
    const getThread = vi.spyOn(mailClient, "getThread").mockResolvedValue({
      ...originalDetail,
      messages: [
        ...Array.from({ length: 5 }, (_, index) => ({
          ...latest,
          id: `outline-${index}`,
          sentAt: `2026-03-0${index + 1}T16:30:00Z`,
          bodyHtml: `<p>Outline body ${index}</p>`,
          bodyText: `Outline snippet ${index}`,
          unread: false,
        })),
        { ...latest, unread: false },
      ],
    });
    const scrollIntoView = vi.fn();
    const originalScroll = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;

    try {
      const { container } = render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      const outline = await screen.findByRole("region", { name: "This thread" });
      fireEvent.click(within(outline).getByRole("button", { name: "Show 3 more" }));
      const target = container.querySelector<HTMLElement>('[data-message-id="outline-0"]')!;
      expect(target).toHaveClass("message-card-collapsed");
      scrollIntoView.mockClear();

      fireEvent.click(within(outline).getByRole("button", { name: /Outline snippet 0/ }));

      await waitFor(() => expect(container.querySelector('[data-message-id="outline-0"]')).toHaveClass("message-card-expanded", "message-active"));
      await waitFor(() => expect(scrollIntoView.mock.contexts).toContain(container.querySelector('[data-message-id="outline-0"]')));
    } finally {
      HTMLElement.prototype.scrollIntoView = originalScroll;
      getThread.mockRestore();
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

      const frameDocuments = screen.getAllByTestId("message-body").map((frame) => (frame as HTMLIFrameElement).srcdoc);

      await advance(3000);
      await screen.findByRole("button", { name: "Mark Unread (u)" });

      expect(container.querySelectorAll("button.message-card-toggle[aria-expanded='false']")).toHaveLength(0);
      expect(container.querySelectorAll("[aria-expanded='true']")).toHaveLength(3);
      expect(screen.getAllByTestId("message-body")).toHaveLength(3);
      expect(screen.getAllByTestId("message-body").map((frame) => (frame as HTMLIFrameElement).srcdoc)).toEqual(frameDocuments);
    } finally {
      getThread.mockRestore();
    }
  });
});

describe("read state and auto-read", () => {
  useConversationFixture();

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
    // The mark-read delay lives on the Appearance page, which Settings opens on.
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

  it.each([false, true])("does not scroll when the delayed auto-read update runs (reply open: %s)", async (replyOpen) => {
    await mailClient.mutateThread({ kind: "read", threadId: "welcome", value: false });
    const scrollIntoView = vi.fn();
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;

    try {
      render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
      scrollIntoView.mockClear();

      if (replyOpen) {
        await act(async () => {
          window.dispatchEvent(new KeyboardEvent("keydown", { key: "r" }));
        });
        await screen.findByRole("dialog", { name: "Reply Message" });
      }

      await advance(3000);
      await screen.findByRole("button", { name: "Mark Unread (u)" });
      expect(scrollIntoView).not.toHaveBeenCalled();
    } finally {
      if (originalScrollIntoView) HTMLElement.prototype.scrollIntoView = originalScrollIntoView;
      else delete (HTMLElement.prototype as Partial<HTMLElement>).scrollIntoView;
    }
  });
});

describe("conversation shortcuts", () => {
  useConversationFixture();

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
});

describe("keyboard-first task and action workspaces", () => {
  beforeEach(() => resetDemoThreads());

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

  it("switches directly between mail and task views while 0 has no effect", async () => {
    let taskStore: ThreadTask[] = [];
    const listTasks = vi.spyOn(mailClient, "listTasks").mockImplementation(async () => taskStore);
    const createTask = vi.spyOn(mailClient, "createTask").mockImplementation(async (request) => {
      const task = {
      id: "task-title-first", accountId: "demo@example.com", threadId: null, sourceMessageId: null,
      subjectSnapshot: null, title: request.title, notes: null, kind: "action", dueKind: "none",
      dueValue: null, timeZone: null, repeatIntervalDays: null, status: "open", completionSource: null,
      evidenceText: null, waitAfter: null, createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null,
      } satisfies ThreadTask;
      taskStore = [task];
      return task;
    });
    try {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    fireEvent.keyDown(window, { key: "3" });
    const workspace = screen.getByRole("region", { name: "Tasks" });
    expect(workspace).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "d" });
    const titleField = await within(workspace).findByRole("textbox", { name: "Task title" });
    expect(titleField).toHaveValue("");
    fireEvent.change(titleField, { target: { value: "Prepare launch notes" } });
    const quickAdd = workspace.querySelector<HTMLElement>(".task-quick-add")!;
    fireEvent.click(within(quickAdd).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(createTask).toHaveBeenCalledWith({
      accountId: "demo@example.com", threadId: null, subjectSnapshot: null, title: "Prepare launch notes", kind: "action",
    }));
    await waitFor(() => expect(workspace.querySelector("#task-task-title-first")).toHaveAttribute("aria-current", "true"));
    expect(within(workspace).getByText("Prepare launch notes", { selector: "strong" })).toBeInTheDocument();
    expect(workspace.querySelector(".task-add-button")).not.toHaveAttribute("title");

    fireEvent.keyDown(window, { key: "0" });
    expect(screen.getByRole("region", { name: "Tasks" })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "1" });
    expect(screen.getByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "0" });
    expect(screen.getByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Tasks" })).not.toBeInTheDocument();
    } finally {
      listTasks.mockRestore();
      createTask.mockRestore();
    }
  });

  it("restores the selected calendar week after switching to mail and back", async () => {
    const { container } = render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    fireEvent.keyDown(window, { key: "2" });
    const week = await screen.findByRole("region", { name: "Calendar week" });
    expect(container.querySelector("main")).toHaveClass("week-open");
    expect(within(week).getByRole("region", { name: "Month picker" })).toBeInTheDocument();
    expect(within(week).getByRole("region", { name: "Calendars" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Inbox" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Conversation" })).not.toBeInTheDocument();

    fireEvent.click(within(week).getByRole("button", { name: "Next week (=)" }));
    const selectedWeekStart = week.querySelector(".calendar-week-day-label")?.textContent;
    const selectedMonth = within(week).getByRole("region", { name: "Month picker" }).querySelector("h3")?.textContent;
    expect(selectedWeekStart).toBeTruthy();
    expect(selectedMonth).toBeTruthy();

    fireEvent.keyDown(window, { key: "1" });
    expect(await screen.findByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Calendar week" })).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "2" });
    const restoredWeek = await screen.findByRole("region", { name: "Calendar week" });
    expect(restoredWeek.querySelector(".calendar-week-day-label")).toHaveTextContent(selectedWeekStart!);
    expect(within(restoredWeek).getByRole("region", { name: "Month picker" })).toHaveTextContent(selectedMonth!);
  });

  it("opens the Contacts Address Book with 4", async () => {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    fireEvent.keyDown(window, { key: "4" });

    expect(await screen.findByRole("heading", { name: "Contacts" })).toBeInTheDocument();
    expect(screen.getByText("Address book")).toBeInTheDocument();
  });

  it("alternates contact views with Tab and remembers the last one when the address book reopens", async () => {
    localStorage.removeItem("threestrands.contacts.view");
    try {
      render(<App />);
      await screen.findByRole("region", { name: "Inbox" });

      fireEvent.keyDown(window, { key: "4" });
      const allTab = await screen.findByRole("tab", { name: "All Contacts" });
      expect(allTab).toHaveAttribute("aria-selected", "true");

      fireEvent.keyDown(document.body, { key: "Tab" });
      expect(await screen.findByRole("tab", { name: /Keep in Touch/ })).toHaveAttribute("aria-selected", "true");
      expect(localStorage.getItem("threestrands.contacts.view")).toBe("keepInTouch");

      // Tab works from the search box too, as it does from mail search.
      fireEvent.keyDown(screen.getByRole("textbox", { name: "Search contacts" }), { key: "Tab", shiftKey: true });
      expect(await screen.findByRole("tab", { name: "All Contacts" })).toHaveAttribute("aria-selected", "true");
      fireEvent.keyDown(document.body, { key: "Tab" });
      expect(await screen.findByRole("tab", { name: /Keep in Touch/ })).toHaveAttribute("aria-selected", "true");

      fireEvent.keyDown(window, { key: "4" });
      await waitFor(() => expect(screen.queryByRole("heading", { name: "Contacts" })).not.toBeInTheDocument());
      fireEvent.keyDown(window, { key: "4" });
      expect(await screen.findByRole("tab", { name: /Keep in Touch/ })).toHaveAttribute("aria-selected", "true");
    } finally {
      localStorage.removeItem("threestrands.contacts.view");
    }
  });

  it("labels the navbar Contacts button with its 4 shortcut and separates it from utility actions", async () => {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    const navbar = screen.getByRole("navigation", { name: "Mailboxes" });
    const contactsButton = within(navbar).getByRole("button", { name: "Contacts (4)" });
    expect(within(navbar).getByRole("tooltip", { name: /Contacts/ })).toHaveTextContent("Contacts4");

    const separator = contactsButton.parentElement?.nextElementSibling;
    expect(separator).toHaveClass("sidebar-nav-separator");
    expect(separator?.nextElementSibling).toContainElement(within(navbar).getByRole("button", { name: "Refresh mail" }));
  });

  it("badges the navbar Contacts button with reminders that are due and opens on Keep in Touch from the palette", async () => {
    const day = 86_400_000;
    const contact = (id: string, dueInDays: number) => ({
      id, displayName: id, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null,
      favorite: false, addresses: [`${id}@example.com`], sentCount: 0, receivedCount: 0, lastInteractedAt: null, birthday: null,
      keepInTouch: { intervalDays: 7, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null },
      keepInTouchDueAt: new Date(Date.now() + dueInDays * day).toISOString(),
    });
    vi.spyOn(mailClient, "listKeepInTouch").mockResolvedValue([contact("overdue", -3), contact("today", 0), contact("later", 20)]);
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    const navbar = screen.getByRole("navigation", { name: "Mailboxes" });
    const contactsButton = await within(navbar).findByRole("button", { name: "Contacts (4), 2 due to reconnect" });
    expect(contactsButton.querySelector(".nav-button-badge")).toHaveTextContent("2");

    fireEvent.keyDown(window, { key: "k", metaKey: true });
    fireEvent.change(await screen.findByRole("textbox", { name: "Filter Commands" }), { target: { value: "Keep in Touch" } });
    fireEvent.click(await screen.findByRole("button", { name: /Go to Keep in Touch/ }));
    expect(await screen.findByRole("tab", { name: /Keep in Touch/ })).toHaveAttribute("aria-selected", "true");
  });

  it("sends the navbar Calendar button to the week calendar view", async () => {
    const { container } = render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    expect(screen.queryByRole("button", { name: "Today’s Schedule (T)" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Calendar (2)" }));
    await screen.findByRole("region", { name: "Calendar week" });
    expect(container.querySelector("main")).toHaveClass("week-open");
    expect(screen.getByRole("button", { name: "Calendar (2)" })).toHaveClass("active");
  });

  it("sends the navbar Inbox button back to the mail view", async () => {
    render(<App />);
    await screen.findByRole("region", { name: "Inbox" });

    const inboxButton = screen.getByRole("button", { name: "Inbox (1)" });
    expect(inboxButton).toHaveClass("active");
    const navbar = screen.getByRole("navigation", { name: "Mailboxes" });
    expect(within(navbar).getByRole("tooltip", { name: /Inbox/ })).toHaveTextContent("Inbox1");

    fireEvent.click(screen.getByRole("button", { name: "Calendar (2)" }));
    await screen.findByRole("region", { name: "Calendar week" });
    expect(screen.queryByRole("region", { name: "Inbox" })).not.toBeInTheDocument();
    expect(inboxButton).not.toHaveClass("active");

    fireEvent.click(inboxButton);
    expect(await screen.findByRole("region", { name: "Inbox" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Calendar week" })).not.toBeInTheDocument();
    expect(inboxButton).toHaveClass("active");
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
    let taskStore: ThreadTask[] = [...tasks];
    listTasks.mockImplementation(async () => taskStore);
    const setStatus = vi.spyOn(mailClient, "setTaskStatus").mockImplementation(async (id) => {
      const updated = { ...taskStore.find((task) => task.id === id)!, status: "completed" as const, completionSource: "user" as const };
      taskStore = taskStore.map((task) => task.id === id ? updated : task);
      return updated;
    });
    const updateTask = vi.spyOn(mailClient, "updateTask").mockImplementation(async (request) => {
      const updated = { ...taskStore.find((task) => task.id === request.id)!, ...request };
      taskStore = taskStore.map((task) => task.id === request.id ? updated : task);
      return updated;
    });

    try {
      const { container } = render(<App />);
      await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

      fireEvent.keyDown(window, { key: "3" });
      const workspace = await screen.findByRole("region", { name: "Tasks" });
      expect(container.querySelector("main")).toHaveClass("tasks-open");
      await waitFor(() => expect(workspace.querySelector("#task-task-welcome")).toHaveAttribute("aria-current", "true"));

      expect(screen.queryByRole("complementary", { name: "Calendar schedule" })).not.toBeInTheDocument();

      fireEvent.keyDown(window, { key: "ArrowDown" });
      await waitFor(() => expect(workspace.querySelector("#task-task-roadmap")).toHaveAttribute("aria-current", "true"));

      fireEvent.keyDown(window, { key: "e" });
      await waitFor(() => expect(setStatus).toHaveBeenCalledWith("task-roadmap", "completed"));

      // The board keeps finished work in its Done column instead of a Completed view.
      fireEvent.click(await within(within(workspace).getByRole("region", { name: "Done" })).findByText("Review the roadmap", { selector: "strong" }));
      await waitFor(() => expect(workspace.querySelector("#task-task-roadmap")).toHaveAttribute("aria-current", "true"));
      // Close the dialog the click opened; Enter reopens it from the keyboard.
      fireEvent.keyDown(window, { key: "Escape" });
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Task details" })).not.toBeInTheDocument());
      fireEvent.keyDown(document.body, { key: "Enter" });
      const details = within(await screen.findByRole("dialog", { name: "Task details" }));
      const titleField = details.getByRole("textbox", { name: "Title" });
      // App shortcuts stay out of the dialog's fields: "e" and "v" are text here.
      fireEvent.keyDown(titleField, { key: "e" });
      fireEvent.keyDown(titleField, { key: "v" });
      expect(setStatus).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("button", { name: "Board", hidden: true })).toHaveAttribute("aria-pressed", "true");
      fireEvent.change(titleField, { target: { value: "Review the project roadmap" } });
      fireEvent.blur(titleField);
      await waitFor(() => expect(updateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "task-roadmap", title: "Review the project roadmap" })));
      fireEvent.change(details.getByRole("combobox", { name: "Due" }), { target: { value: "date" } });
      fireEvent.change(details.getByLabelText("Due date"), { target: { value: "2026-09-30" } });
      fireEvent.keyDown(details.getByLabelText("Due date"), { key: "Escape" });
      await waitFor(() => expect(updateTask).toHaveBeenCalledWith(expect.objectContaining({ id: "task-roadmap", dueKind: "date", dueValue: "2026-09-30" })));
      await waitFor(() => expect(screen.queryByRole("dialog", { name: "Task details" })).not.toBeInTheDocument());

      fireEvent.keyDown(document.body, { key: "o" });
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
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", shiftKey: true }));
    });
    expect(await screen.findByRole("option", { name: /Welcome to ThreeStrands/ })).toBeInTheDocument();
    expect(await screen.findByRole("option", { name: /Phase 1: read and triage/ })).toBeInTheDocument();
  });

  it("multi-selects rows with Command-click and Shift-click, and a plain click resets", async () => {
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const rows = () => [...container.querySelectorAll<HTMLButtonElement>(".thread-row")];
    expect(rows().length).toBeGreaterThanOrEqual(3);

    fireEvent.click(rows()[0]);
    fireEvent.click(rows()[2], { metaKey: true });
    expect(await screen.findByText("2 selected")).toBeInTheDocument();
    expect(rows()[1]).not.toHaveTextContent("Selected for batch actions");

    fireEvent.click(rows()[2], { metaKey: true });
    expect(await screen.findByText("1 selected")).toBeInTheDocument();

    fireEvent.click(rows()[0]);
    expect(screen.queryByRole("toolbar", { name: "Batch actions" })).not.toBeInTheDocument();

    fireEvent.click(rows()[2], { shiftKey: true });
    expect(await screen.findByText("3 selected")).toBeInTheDocument();

    fireEvent.click(rows()[1]);
    expect(screen.queryByRole("toolbar", { name: "Batch actions" })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Phase 1: read and triage" })).toBeInTheDocument();
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
  beforeEach(() => resetDemoThreads());

  afterEach(cleanup);

  it("closes the composer when focus is in a field", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "New message (c)" }));

    const recipient = await screen.findByRole("textbox", { name: "To" });
    recipient.focus();
    fireEvent.keyDown(recipient, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "New Message" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("region", { name: "Conversation" })).toBeInTheDocument();
  });

  it("closes only the topmost popup when overlays are stacked", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "New message (c)" }));
    const composer = await screen.findByRole("dialog", { name: "New Message" });
    fireEvent.click(screen.getByRole("button", { name: "Command Palette (⌘K)" }));

    const filter = await screen.findByRole("textbox", { name: "Filter Commands" });
    fireEvent.keyDown(filter, { key: "Escape" });

    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Command Palette" })).not.toBeInTheDocument(),
    );
    expect(composer).toBeInTheDocument();
  });
});

describe("Drafts folder", () => {
  beforeEach(() => resetDemoThreads());

  afterEach(cleanup);

  it("switches to the inline Drafts view and back to the inbox without losing state", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    selectFolder("Drafts");

    expect(await screen.findByRole("heading", { name: "0 drafts" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Welcome to ThreeStrands" })).not.toBeInTheDocument();

    selectFolder("Inbox");
    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
  });

  it("discards drafts with # from the Drafts list and from the open draft", async () => {
    await mailClient.createDraft("new");
    await mailClient.createDraft("new");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    selectFolder("Drafts");
    expect(await screen.findByRole("heading", { name: "2 drafts" })).toBeInTheDocument();

    const [first] = screen.getAllByRole("button", { name: /\(no subject\)/ });
    first.focus();
    fireEvent.keyDown(first, { key: "#", code: "Digit3", shiftKey: true });
    expect(await screen.findByRole("heading", { name: "1 drafts" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /\(no subject\)/ }));
    const discard = await screen.findByRole("button", { name: "Discard Draft" });
    discard.focus();
    fireEvent.keyDown(discard, { key: "#", code: "Digit3", shiftKey: true });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Discard Draft" })).not.toBeInTheDocument());
    expect(await screen.findByRole("heading", { name: "0 drafts" })).toBeInTheDocument();

    selectFolder("Inbox");
    expect(await screen.findByRole("heading", { name: "Welcome to ThreeStrands" })).toBeInTheDocument();
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

describe("search debounce", () => {
  useConversationFixture();

  it("stays debounced once the outbox holds delivered history", async () => {
    const sentDraft = {
      id: "sent-draft", revision: 1, account: "demo@example.com", mode: "new", sourceId: null, threadId: null,
      replyId: null, references: [], to: "a@example.com", cc: "", bcc: "", subject: "Sent", body: "Hi",
      followUpTaskId: null, attachments: [], updatedAt: 0,
    };
    localStorage.setItem("threestrands.demoCorrespondence", JSON.stringify({
      drafts: [],
      outbox: [{ id: "sent-1", draft: sentDraft, state: "sent", deadline: 0, error: null }],
    }));
    const searchThreads = vi.spyOn(mailClient, "searchThreads");
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    await advance(50);

    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    searchThreads.mockClear();
    await act(async () => {
      fireEvent.change(search, { target: { value: "r" } });
      fireEvent.change(search, { target: { value: "ro" } });
      fireEvent.change(search, { target: { value: "roa" } });
    });
    expect(searchThreads).not.toHaveBeenCalled();

    await advance(200);
    await waitFor(() => expect(searchThreads).toHaveBeenCalledTimes(1));
    expect(searchThreads).toHaveBeenCalledWith(expect.objectContaining({ query: "roa" }), undefined);
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
    const workAccount = await screen.findByRole("radio", { name: "Work" });
    fireEvent.click(workAccount);
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("work@example.com");
    expect(firstRun.container.querySelector(".mailbox-heading-context")).toHaveTextContent("work@example.com");
    fireEvent.click(screen.getByRole("button", { name: "Tasks (3)" }));
    expect(firstRun.container.querySelector(".tasks-sidebar-header")).toHaveTextContent("work@example.com");
    fireEvent.click(screen.getByRole("button", { name: "Contacts (4)" }));
    expect(firstRun.container.querySelector(".contacts-header")).toHaveTextContent("work@example.com");
    firstRun.unmount();

    const secondRun = render(<App />);
    await waitFor(() => expect(screen.getByRole("radio", { name: "Work" })).toHaveAttribute("aria-checked", "true"));
    fireEvent.click(screen.getByRole("radio", { name: /All accounts/i }));
    expect(localStorage.getItem("threestrands.settings.selectedAccountId")).toBe("all");
    expect(secondRun.container.querySelector(".mailbox-heading-context")).toHaveTextContent("All accounts");
    secondRun.unmount();

    render(<App />);
    await waitFor(() => expect(screen.getByRole("radio", { name: /All accounts/i })).toHaveAttribute("aria-checked", "true"));
  });

  it("creates a task for the chosen account from the combined task view", async () => {
    const [primary] = await mailClient.listAccounts();
    vi.spyOn(mailClient, "listAccounts").mockResolvedValue([primary!, { ...primary!, email: "work@example.com", displayName: "Work", sortOrder: 1 }]);
    const createTask = vi.spyOn(mailClient, "createTask").mockResolvedValue({
      id: "chosen-account-task", accountId: "work@example.com", threadId: null, sourceMessageId: null,
      subjectSnapshot: null, title: "Review plan", notes: null, kind: "action", dueKind: "none",
      dueValue: null, timeZone: null, repeatIntervalDays: null, status: "open", completionSource: null,
      evidenceText: null, waitAfter: null, createdAt: "2026-09-19T10:00:00Z", updatedAt: "2026-09-19T10:00:00Z", completedAt: null,
    } satisfies ThreadTask);
    localStorage.setItem("threestrands.settings.selectedAccountId", "all");
    render(<App />);
    expect(await screen.findByRole("region", { name: "Inbox" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Tasks (3)" }));
    const workspace = screen.getByRole("region", { name: "Tasks" });
    expect(workspace.querySelector(".tasks-sidebar-header")).toHaveTextContent("All accounts");
    fireEvent.click(within(workspace).getByRole("button", { name: "Add Task" }));
    const form = workspace.querySelector<HTMLElement>(".task-quick-add")!;
    fireEvent.change(within(form).getByRole("textbox", { name: "Task title" }), { target: { value: "Review plan" } });
    expect(within(form).getByRole("button", { name: "Add task" })).toBeDisabled();
    fireEvent.change(within(form).getByRole("combobox", { name: "Account" }), { target: { value: "work@example.com" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(createTask).toHaveBeenCalledWith({ accountId: "work@example.com", threadId: null, subjectSnapshot: null, title: "Review plan", kind: "action" }));
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
        status: "needs_reauth" as const,
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
    expect(screen.getByRole("tooltip", { name: "work@example.com · Needs reconnect in Mail Accounts" })).toBeInTheDocument();
    const disconnectedAccount = screen.getByRole("radio", { name: "Work, 120 unread, Needs reconnect" });
    expect(disconnectedAccount).toHaveTextContent("99+");
    expect(disconnectedAccount.querySelector(".account-reconnect-badge")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: `${primary.email}, 3 unread` }).querySelector(".account-reconnect-badge")).not.toBeInTheDocument();
  });

  it("shows a lone account as checked, with its email tooltip and no All accounts option", async () => {
    const [account] = await mailClient.listAccounts();
    render(
      <AccountSwitcher
        accounts={[account!]}
        unreadCounts={{}}
        activeAccountId={null}
        onSwitch={() => {}}
        onShowAll={() => {}}
        onReorder={() => {}}
      />,
    );

    expect(screen.getByRole("radio", { name: account!.email })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("tooltip", { name: account!.email, hidden: true })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: /^All accounts/i })).not.toBeInTheDocument();
  });
});

describe("returning from a jump to another conversation", () => {
  useConversationFixture();

  async function jumpFromRecentEmails() {
    const roadmap = await mailClient.getThread("roadmap");
    vi.spyOn(mailClient, "contactTimeline").mockResolvedValue([{
      threadId: "roadmap",
      accountId: roadmap.thread.accountId,
      contactEmail: "team@example.com",
      subject: roadmap.thread.subject,
      snippet: "",
      sentAt: roadmap.thread.lastMessageAt,
      labels: [],
    }]);
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    const recent = await screen.findByRole("region", { name: "Recent emails" });
    fireEvent.click(within(recent).getByRole("button", { name: new RegExp(roadmap.thread.subject) }));
    await screen.findByRole("heading", { name: roadmap.thread.subject });
    return roadmap;
  }

  it("offers Back to the conversation the jump started from, by link or Escape", async () => {
    await jumpFromRecentEmails();
    const back = screen.getByRole("button", { name: /Back to Welcome to ThreeStrands/ });
    expect(back).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    // Back is spent once used, so Escape does nothing more.
    expect(screen.queryByRole("button", { name: /Back to/ })).not.toBeInTheDocument();
  });

  it("returns to the origin's search", async () => {
    const roadmap = await mailClient.getThread("roadmap");
    vi.spyOn(mailClient, "contactTimeline").mockResolvedValue([{
      threadId: "roadmap", accountId: roadmap.thread.accountId, contactEmail: "team@example.com",
      subject: roadmap.thread.subject, snippet: "", sentAt: roadmap.thread.lastMessageAt, labels: [],
    }]);
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.keyDown(window, { key: "/" });
    const search = await screen.findByRole("textbox", { name: "Search Mail" });
    fireEvent.change(search, { target: { value: "Welcome" } });
    // The search commits once typing pauses.
    await advance(SEARCH_DEBOUNCE_MS);
    const recent = await screen.findByRole("region", { name: "Recent emails" });
    fireEvent.click(within(recent).getByRole("button", { name: new RegExp(roadmap.thread.subject) }));
    await screen.findByRole("heading", { name: roadmap.thread.subject });
    // The jump clears the search; Back brings it back.
    expect(screen.queryByRole("textbox", { name: "Search Mail" })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Back to Welcome to ThreeStrands/ }));
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    expect(await screen.findByRole("textbox", { name: "Search Mail" })).toHaveValue("Welcome");
  });

  it("forgets the jump once the user picks another conversation", async () => {
    await jumpFromRecentEmails();
    fireEvent.keyDown(window, { key: "j" });
    await waitFor(() => expect(screen.queryByRole("button", { name: /Back to/ })).not.toBeInTheDocument());
  });
});
