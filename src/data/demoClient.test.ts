import { afterEach, describe, expect, it } from "vitest";
import { DEMO_ACCOUNT_ID, demoClient } from "./demoClient";

const OTHER_ACCOUNT_ID = "other@example.com";

describe("demoClient split inbox exclusion", () => {
  afterEach(async () => {
    const splits = await demoClient.listSplitInboxes();
    await Promise.all(splits.map((split) => demoClient.deleteSplitInbox(split.id)));
  });

  it("pulls a split-matched thread out of the Inbox listing", async () => {
    const before = await demoClient.listThreads();
    expect(before.some((thread) => thread.id === "roadmap")).toBe(true);

    await demoClient.createSplitInbox("Product", "pattern", "product", DEMO_ACCOUNT_ID);

    const after = await demoClient.listThreads();
    expect(after.some((thread) => thread.id === "roadmap")).toBe(false);
    expect(after.some((thread) => thread.id === "welcome")).toBe(true);
  });

  it("still surfaces the split-matched thread from its own split inbox page", async () => {
    const split = await demoClient.createSplitInbox("Product", "pattern", "product", DEMO_ACCOUNT_ID);
    const page = await demoClient.listSplitInboxPage(split.id, 0, 10);
    expect(page.threads.map((thread) => thread.id)).toContain("roadmap");
  });

  it("moves a matched unread thread's count from the Inbox bucket into its split", async () => {
    const before = await demoClient.mailboxUnreadCounts();
    expect(before.inbox).toBeGreaterThan(0);
    expect(before.splits).toEqual({});

    // "welcome" (unread, participant "Dispatch") is the only seeded thread this rule matches.
    const split = await demoClient.createSplitInbox("Dispatch", "pattern", "dispatch", DEMO_ACCOUNT_ID);
    const after = await demoClient.mailboxUnreadCounts();
    expect(after.inbox).toBe(before.inbox - 1);
    expect(after.splits[split.id]).toBe(1);
  });

  it("never excludes or counts a match belonging to a different account", async () => {
    const before = await demoClient.listThreads();
    const beforeCounts = await demoClient.mailboxUnreadCounts();

    // Matches "roadmap" and "welcome" respectively, but the rules are owned
    // by an account with no threads, so neither should have any effect.
    const productSplit = await demoClient.createSplitInbox("Product", "pattern", "product", OTHER_ACCOUNT_ID);
    await demoClient.createSplitInbox("Dispatch", "pattern", "dispatch", OTHER_ACCOUNT_ID);

    const after = await demoClient.listThreads();
    expect(after.map((thread) => thread.id)).toEqual(before.map((thread) => thread.id));

    const afterCounts = await demoClient.mailboxUnreadCounts();
    expect(afterCounts.inbox).toBe(beforeCounts.inbox);
    expect(afterCounts.splits[productSplit.id]).toBeUndefined();

    const page = await demoClient.listSplitInboxPage(productSplit.id, 0, 10);
    expect(page.threads).toEqual([]);
  });
});
