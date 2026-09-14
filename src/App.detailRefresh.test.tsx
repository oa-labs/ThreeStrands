import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App, messagesWithQueuedReplies } from "./App";
import { mailClient } from "./data/client";
import type { OutboxItem } from "./correspondence";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("puts 'and' before the final message recipient", async () => {
  const originalGetThread = mailClient.getThread.bind(mailClient);
  vi.spyOn(mailClient, "getThread").mockImplementation(async (id) => {
    const detail = await originalGetThread(id);
    return id === "welcome" ? {
      ...detail,
      messages: detail.messages.map((message) => ({
        ...message,
        recipients: ["Joel Reed <joel@example.com>", "Leann Moore <leann@example.com>", "Cara Cenfetelli <cara@example.com>"],
      })),
    } : detail;
  });

  render(<App />);
  await screen.findByRole("heading", { name: "Welcome to Dispatch" });

  const recipientLine = document.querySelector(".message-recipients");
  expect(recipientLine?.querySelectorAll(".address-name")).toHaveLength(3);
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
        recipients: ["hello@dispatch.local"],
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
  await screen.findByRole("heading", { name: "Welcome to Dispatch" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh mail" }));

  await waitFor(() => {
    const bodies = screen.getAllByTestId("message-body") as HTMLIFrameElement[];
    expect(bodies.some((body) => body.srcdoc.includes("Sent reply body"))).toBe(true);
  });
  expect(screen.getByText("Joel", { selector: ".address-name" })).toBeInTheDocument();
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
      attachments: [],
      updatedAt: Date.now(),
    },
  };

  const optimistic = messagesWithQueuedReplies(detail, [queued]);
  expect(optimistic).toHaveLength(detail.messages.length + 1);
  expect(optimistic.at(-1)).toMatchObject({
    id: "outbox-queued-reply",
    bodyText: "Immediate reply",
    recipients: ['"Doe, Jane" <jane@example.com>', "brian@example.com", "team@example.com"],
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

it("renders a reply in the open conversation as soon as Send queues it", async () => {
  localStorage.removeItem("dispatch.demoCorrespondence");
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "Welcome to Dispatch" });
    fireEvent.click(screen.getByRole("button", { name: "Reply (r)" }));

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
    localStorage.removeItem("dispatch.demoCorrespondence");
  }
});
