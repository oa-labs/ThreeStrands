import { describe, expect, it } from "vitest";
import { filterThreadsByMessageFilters, type MessageFilterKind } from "./messageFilters";
import type { Thread } from "./domain";

function makeThread(overrides: Partial<Thread> & { id: string }): Thread {
  return {
    providerThreadId: overrides.id,
    subject: "",
    snippet: "",
    participants: [],
    lastMessageAt: "2024-01-01T00:00:00Z",
    lastReceivedAt: "2024-01-01T00:00:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: [],
    accountId: "a@example.com",
    summary: null,
    summaryGeneratedAt: null,
    hasAttachments: false,
    ...overrides,
  };
}

describe("filterThreadsByMessageFilters", () => {
  const threads = [
    makeThread({ id: "unread-only", unread: true }),
    makeThread({ id: "starred-only", starred: true }),
    makeThread({ id: "important-only", labels: ["IMPORTANT"] }),
    makeThread({ id: "replied", labels: ["SENT"] }),
    makeThread({ id: "no-reply-and-unread", unread: true }),
    makeThread({ id: "plain" }),
  ];

  it("returns every thread when no filters are active", () => {
    expect(filterThreadsByMessageFilters(threads, new Set())).toEqual(threads);
  });

  it("keeps only unread threads", () => {
    const result = filterThreadsByMessageFilters(threads, new Set<MessageFilterKind>(["unread"]));
    expect(result.map((thread) => thread.id)).toEqual(["unread-only", "no-reply-and-unread"]);
  });

  it("keeps only starred threads", () => {
    const result = filterThreadsByMessageFilters(threads, new Set<MessageFilterKind>(["starred"]));
    expect(result.map((thread) => thread.id)).toEqual(["starred-only"]);
  });

  it("keeps only threads carrying Gmail's IMPORTANT label", () => {
    const result = filterThreadsByMessageFilters(threads, new Set<MessageFilterKind>(["important"]));
    expect(result.map((thread) => thread.id)).toEqual(["important-only"]);
  });

  it("treats a thread as \"no reply\" only when none of its messages carry the SENT label", () => {
    const result = filterThreadsByMessageFilters(threads, new Set<MessageFilterKind>(["noReply"]));
    expect(result.map((thread) => thread.id)).not.toContain("replied");
    expect(result.map((thread) => thread.id)).toContain("plain");
  });

  it("combines multiple active filters with AND semantics", () => {
    const result = filterThreadsByMessageFilters(
      threads,
      new Set<MessageFilterKind>(["unread", "noReply"]),
    );
    expect(result.map((thread) => thread.id)).toEqual(["unread-only", "no-reply-and-unread"]);
  });
});
