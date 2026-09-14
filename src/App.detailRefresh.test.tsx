import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { mailClient } from "./data/client";

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
