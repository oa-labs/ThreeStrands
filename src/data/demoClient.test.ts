import { afterEach, describe, expect, it } from "vitest";
import { demoClient } from "./demoClient";

describe("demoClient split inbox exclusion", () => {
  afterEach(async () => {
    const splits = await demoClient.listSplitInboxes();
    await Promise.all(splits.map((split) => demoClient.deleteSplitInbox(split.id)));
  });

  it("pulls a split-matched thread out of the Inbox listing", async () => {
    const before = await demoClient.listThreads();
    expect(before.some((thread) => thread.id === "roadmap")).toBe(true);

    await demoClient.createSplitInbox("Product", "pattern", "product");

    const after = await demoClient.listThreads();
    expect(after.some((thread) => thread.id === "roadmap")).toBe(false);
    expect(after.some((thread) => thread.id === "welcome")).toBe(true);
  });

  it("still surfaces the split-matched thread from its own split inbox page", async () => {
    const split = await demoClient.createSplitInbox("Product", "pattern", "product");
    const page = await demoClient.listSplitInboxPage(split.id, undefined, 0, 10);
    expect(page.threads.map((thread) => thread.id)).toContain("roadmap");
  });

  it("moves a matched unread thread's count from the Inbox bucket into its split", async () => {
    const before = await demoClient.mailboxUnreadCounts();
    expect(before.inbox).toBeGreaterThan(0);
    expect(before.splits).toEqual({});

    // "welcome" (unread, participant "Dispatch") is the only seeded thread this rule matches.
    const split = await demoClient.createSplitInbox("Dispatch", "pattern", "dispatch");
    const after = await demoClient.mailboxUnreadCounts();
    expect(after.inbox).toBe(before.inbox - 1);
    expect(after.splits[split.id]).toBe(1);
  });
});
