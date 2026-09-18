import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App, formatMailTimestamp, messagesWithQueuedReplies } from "./App";
import { mailClient } from "./data/client";
import type { OutboxItem } from "./correspondence";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("formats today's mail with the time of day and older mail with the date", () => {
  const now = new Date(2026, 8, 14, 18, 0);
  const today = new Date(2026, 8, 14, 9, 5);
  const older = new Date(2026, 8, 13, 23, 55);

  expect(formatMailTimestamp(today.toISOString(), now)).toBe(new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  }).format(today));
  expect(formatMailTimestamp(older.toISOString(), now)).toBe(new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "2-digit",
  }).format(older));
});

it("puts 'and' before the final message recipient", async () => {
  const originalGetThread = mailClient.getThread.bind(mailClient);
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    return id === "welcome" ? {
      ...detail,
      messages: detail.messages.map((message) => ({
        ...message,
        recipients: [
          "Joel Reed <joel@example.com>",
          '"Bates',
          'Daniel R" <daniel@example.com>',
          "Cara Cenfetelli <cara@example.com>",
        ],
      })),
    } : detail;
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

  const recipientLine = document.querySelector(".message-recipients");
  expect(recipientLine?.querySelectorAll(".address-name")).toHaveLength(3);
  expect(Array.from(recipientLine?.querySelectorAll(".address-name") ?? [], ({ textContent }) => textContent))
    .toEqual(["Joel Reed", "Bates, Daniel R", "Cara Cenfetelli"]);
  expect(Array.from(recipientLine?.children ?? []).map((recipient) =>
    Array.from(recipient.childNodes).find((node) => node.nodeType === Node.TEXT_NODE)?.textContent ?? "",
  )).toEqual(["", ", ", ", and "]);
});

it("refreshes the open conversation when its inbox row receives a sent reply", async () => {
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  const originalGetThread = mailClient.getThread.bind(mailClient);
  const originalListAccounts = mailClient.listAccounts.bind(mailClient);
  const syncStatus = await mailClient.syncStatus();
  let replyDelivered = false;

  vi.spyOn(mailClient, "sync").mockImplementation(async () => {
    replyDelivered = true;
    return syncStatus;
  });
  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () =>
    (await originalListAccounts()).map((account) => account.email === "demo@example.com"
      ? { ...account, displayName: "Joel Reed" }
      : account),
  );
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    const page = await originalList(...args);
    if (!replyDelivered) return page;
    return {
      ...page,
      threads: page.threads.map((thread) => thread.id === "welcome" ? {
        ...thread,
        snippet: "Sent reply body",
        lastMessageAt: "2026-03-05T17:30:00Z",
      } : thread),
    };
  });
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    if (!replyDelivered || id !== "welcome") return detail;
    return {
      ...detail,
      thread: {
        ...detail.thread,
        snippet: "Sent reply body",
        lastMessageAt: "2026-03-05T17:30:00Z",
      },
      messages: [...detail.messages, {
        id: "sent-reply",
        threadId: "welcome",
        sender: "<demo@example.com>",
        recipients: ["hello@threestrands.local"],
        sentAt: "2026-03-05T17:30:00Z",
        bodyHtml: "<p>Sent reply body</p>",
        bodyText: "Sent reply body",
        unread: false,
        unsubscribe: null,
        attachments: [],
      }],
    };
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

  await waitFor(() => {
    const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
    expect(bodies.some((body) => body.srcdoc.includes("Sent reply body"))).toBe(true);
  });
  expect(screen.getByText("Joel Reed", { selector: ".address-name" })).toBeInTheDocument();
});

it("reloads the local inbox after a refresh even when one account sync fails", async () => {
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  const originalStatus = mailClient.syncStatus.bind(mailClient);
  let listCalls = 0;

  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    listCalls += 1;
    return originalList(...args);
  });
  vi.spyOn(mailClient, "sync").mockRejectedValue(new Error("second@example.com failed"));
  vi.spyOn(mailClient, "syncStatus").mockImplementation(originalStatus);

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  const callsBeforeRefresh = listCalls;

  fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

  await waitFor(() => expect(listCalls).toBeGreaterThan(callsBeforeRefresh));
});

it("reloads the inbox after reconnecting an imported account", async () => {
  const originalAccounts = mailClient.listAccounts.bind(mailClient);
  const originalList = mailClient.listThreadsPage.bind(mailClient);
  let needsReconnect = true;
  let listCalls = 0;

  vi.spyOn(mailClient, "listAccounts").mockImplementation(async () =>
    (await originalAccounts()).map((account) => ({
      ...account,
      status: needsReconnect ? "needs_reauth" as const : "connected" as const,
    })),
  );
  vi.spyOn(mailClient, "reconnectAccount").mockImplementation(async (email) => {
    needsReconnect = false;
    return (await originalAccounts()).find((account) => account.email === email)!;
  });
  vi.spyOn(mailClient, "listThreadsPage").mockImplementation(async (...args) => {
    listCalls += 1;
    return originalList(...args);
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
  fireEvent.click(screen.getByRole("button", { name: "Settings (⌘,)" }));
  const dialog = screen.getByRole("dialog", { name: "Settings" });
  fireEvent.click(within(dialog).getByRole("button", { name: "Mail Accounts" }));
  const callsBeforeReconnect = listCalls;

  fireEvent.click(await within(dialog).findByRole("button", { name: "Reconnect" }));

  await waitFor(() => expect(mailClient.reconnectAccount).toHaveBeenCalled());
  await waitFor(() => expect(listCalls).toBeGreaterThan(callsBeforeReconnect));
  expect(within(dialog).getByText("Connected")).toBeInTheDocument();
});

it("shows a queued reply immediately and replaces it with the provider copy", async () => {
  const detail = await mailClient.getThread("welcome");
  const source = detail.messages.at(-1)!;
  const queued: OutboxItem = {
    id: "queued-reply",
    state: "undo_pending",
    deadline: Date.now() + 10_000,
    error: null,
    draft: {
      id: "reply-draft",
      revision: 2,
      account: "demo@example.com",
      mode: "reply",
      sourceId: source.id,
      threadId: detail.thread.providerThreadId,
      replyId: "source@example.com",
      references: [],
      to: '"Doe, Jane" <jane@example.com>, brian@example.com',
      cc: "team@example.com",
      bcc: "",
      subject: detail.thread.subject,
      body: "Immediate reply",
      bodyHtml: "<p>Immediate reply</p>",
      attachments: [{
        id: "inline-1",
        name: "image.png",
        mime: "image/png",
        size: 4,
        ready: true,
        messageId: null,
        providerId: null,
        inline: true,
        contentId: "inline-1@threestrands.local",
      }],
      updatedAt: Date.now(),
    },
  };

  const optimistic = messagesWithQueuedReplies(detail, [queued]);
  expect(optimistic).toHaveLength(detail.messages.length + 1);
  expect(optimistic.at(-1)).toMatchObject({
    id: "outbox-queued-reply",
    bodyText: "Immediate reply",
    recipients: ['"Doe, Jane" <jane@example.com>', "brian@example.com", "team@example.com"],
    attachments: [{
      id: "inline-1",
      filename: "image.png",
      mimeType: "image/png",
      inline: true,
      contentId: "inline-1@threestrands.local",
    }],
  });

  const providerDetail = {
    ...detail,
    messages: [...detail.messages, {
      ...optimistic.at(-1)!,
      id: "gmail-sent-reply",
      sentAt: new Date().toISOString(),
    }],
  };
  expect(messagesWithQueuedReplies(providerDetail, [{ ...queued, state: "sent", providerId: "gmail-sent-reply" }]))
    .toHaveLength(providerDetail.messages.length);
  expect(messagesWithQueuedReplies(detail, [{ ...queued, state: "canceled" }]))
    .toHaveLength(detail.messages.length);
});

it("resolves a queued reply's inline image from its outbox draft", async () => {
  const detail = await mailClient.getThread("welcome");
  const source = detail.messages.at(-1)!;
  const queued: OutboxItem = {
    id: "queued-inline-reply",
    state: "undo_pending",
    deadline: Date.now() + 10_000,
    error: null,
    draft: {
      id: "inline-reply-draft",
      revision: 2,
      account: "demo@example.com",
      mode: "reply",
      sourceId: source.id,
      threadId: detail.thread.providerThreadId,
      replyId: "source@example.com",
      references: [],
      to: "hello@threestrands.local",
      cc: "",
      bcc: "",
      subject: detail.thread.subject,
      body: "Screenshot",
      bodyHtml: '<p>Screenshot</p><img src="cid:inline-1@threestrands.local" alt="image.png">',
      attachments: [{
        id: "inline-1",
        name: "image.png",
        mime: "image/png",
        size: 4,
        ready: true,
        messageId: null,
        providerId: null,
        inline: true,
        contentId: "inline-1@threestrands.local",
      }],
      updatedAt: Date.now(),
    },
  };
  vi.spyOn(mailClient, "listOutbox").mockResolvedValue([queued]);
  vi.spyOn(mailClient, "listDrafts").mockResolvedValue([]);
  const readInline = vi.spyOn(mailClient, "readInlineImage")
    .mockResolvedValue("data:image/png;base64,iVBORw==");

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });

  await waitFor(() => expect(readInline).toHaveBeenCalledWith("inline-reply-draft", "inline-1"));
  await waitFor(() => {
    const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
    expect(bodies.some((body) => body.srcdoc.includes("data:image/png;base64,iVBORw=="))).toBe(true);
  });
});

it("renders a reply in the open conversation as soon as Send queues it", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Reply" }));

    const editor = await screen.findByRole("textbox", { name: "Message body" });
    editor.innerHTML = "<p>Visible without waiting for delivery</p>";
    fireEvent.input(editor);
    fireEvent.click(screen.getByRole("button", { name: /Send/ }));

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply message" })).not.toBeInTheDocument());
    await waitFor(() => {
      const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
      expect(bodies.some((body) => body.srcdoc.includes("Visible without waiting for delivery"))).toBe(true);
    });
    expect(screen.getByRole("status")).toHaveTextContent("Sending in");
  } finally {
    localStorage.removeItem("threestrands.demoCorrespondence");
  }
});

it("sends and marks the open conversation done with Mod+Shift+Enter", async () => {
  localStorage.removeItem("threestrands.demoCorrespondence");
  const mutateThreads = vi.spyOn(mailClient, "mutateThreads").mockResolvedValue();
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to ThreeStrands" });
    fireEvent.click(screen.getByRole("button", { name: "Reply" }));

    const editor = await screen.findByRole("textbox", { name: "Message body" });
    editor.innerHTML = "<p>Send this and mark the conversation done</p>";
    fireEvent.input(editor);
    fireEvent.keyDown(editor, { key: "Enter", metaKey: true, shiftKey: true });

    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Reply message" })).not.toBeInTheDocument());
    await waitFor(() => expect(mutateThreads).toHaveBeenCalledWith([
      { kind: "archive", threadId: "welcome", value: true },
    ]));
  } finally {
    localStorage.removeItem("threestrands.demoCorrespondence");
  }
});
