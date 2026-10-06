import { afterEach, describe, expect, it, vi } from "vitest";
import { hasEmailedBefore, MIN_PROACTIVE_DWELL_SECONDS, proactiveBriefSender, proactiveDwellMs } from "./proactiveBrief";
import { mailClient } from "./data/client";
import type { ContactProfile, Message, ThreadDetail } from "./domain";

const own = new Set(["you@example.com", "work@example.com"]);

function message(id: string, sender: string, overrides: Partial<Message> = {}): Message {
  return {
    id, threadId: "thread-1", sender, recipients: ["you@example.com"], sentAt: "2026-09-19T10:00:00Z",
    bodyHtml: "", bodyText: "Hello", unread: false, unsubscribe: null, attachments: [], ...overrides,
  } as Message;
}

function thread(...messages: Message[]): ThreadDetail {
  return { thread: { id: "thread-1" }, messages } as unknown as ThreadDetail;
}

function contact(sentCount: number, addresses: string[]): ContactProfile {
  return {
    id: "contact:1", displayName: "Jane", role: null, company: null, location: null, bio: null, notes: null,
    links: [], photoData: null, favorite: false, addresses, sentCount, receivedCount: 1, lastInteractedAt: null, birthday: null, keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null,
  };
}

describe("proactive brief rules", () => {
  afterEach(() => vi.restoreAllMocks());

  it("waits for the mark-read delay but never less than the minimum dwell", () => {
    expect(proactiveDwellMs(0)).toBe(MIN_PROACTIVE_DWELL_SECONDS * 1000);
    expect(proactiveDwellMs(MIN_PROACTIVE_DWELL_SECONDS - 1)).toBe(MIN_PROACTIVE_DWELL_SECONDS * 1000);
    expect(proactiveDwellMs(MIN_PROACTIVE_DWELL_SECONDS)).toBe(MIN_PROACTIVE_DWELL_SECONDS * 1000);
    expect(proactiveDwellMs(MIN_PROACTIVE_DWELL_SECONDS + 1)).toBe((MIN_PROACTIVE_DWELL_SECONDS + 1) * 1000);
  });

  it("briefs a conversation about its newest outside sender", () => {
    expect(proactiveBriefSender(thread(
      message("1", "Jane <Jane@Example.com>"),
      message("2", "Me <you@example.com>"),
      message("3", "Bob <bob@example.com>"),
      message("4", "work@example.com"),
    ), own)).toBe("bob@example.com");
  });

  it("skips mailing lists and conversations with no one else in them", () => {
    const unsubscribe = { methods: [{ kind: "mailto", target: "leave@list.example.com" }] } as unknown as Message["unsubscribe"];
    expect(proactiveBriefSender(thread(message("1", "News <news@list.example.com>", { unsubscribe })), own)).toBeNull();
    expect(proactiveBriefSender(thread(message("1", "Jane <jane@example.com>"), message("2", "news@list.example.com", { unsubscribe })), own)).toBeNull();
    expect(proactiveBriefSender(thread(message("1", "you@example.com"), message("2", "Work <work@example.com>")), own)).toBeNull();
    expect(proactiveBriefSender(thread(message("1", "Jane <jane@example.com>", { unsubscribe: { methods: [] } as unknown as Message["unsubscribe"] })), own)).toBe("jane@example.com");
  });

  it("recognizes senders the user has emailed, matching the address exactly", async () => {
    const list = vi.spyOn(mailClient, "listContactProfiles");
    list.mockResolvedValueOnce([contact(2, ["jane@example.com"])]);
    expect(await hasEmailedBefore("Jane@Example.com")).toBe(true);
    expect(list).toHaveBeenLastCalledWith("jane@example.com", 20);
    list.mockResolvedValueOnce([contact(0, ["jane@example.com"])]);
    expect(await hasEmailedBefore("jane@example.com")).toBe(false);
    list.mockResolvedValueOnce([contact(5, ["jane.doe@example.com"])]);
    expect(await hasEmailedBefore("jane@example.com")).toBe(false);
  });
});
