import { describe, expect, it } from "vitest";
import { createDemoClient } from "./demoClient";
import { defaultDemoDataset, DEMO_ACCOUNT_ID } from "./demoDataset";

describe("demo inbox assistants", () => {
  it("lists all three assistants disabled by default and validates config updates", async () => {
    const client = createDemoClient(defaultDemoDataset());
    const initial = await client.getInboxAgentsSnapshot();
    expect(initial.runnerEnabled).toBe(true);
    expect(initial.agents).toHaveLength(3);
    expect(initial.agents.every((a) => !a.enabled)).toBe(true);

    await expect(
      client.updateInboxAgentConfig({
        agentId: "stale_reads",
        enabled: true,
        ageDays: 0,
        maxThreadsPerRun: 10,
        includeStarred: false,
        accountId: null,
        scheduleHours: null,
      }),
    ).rejects.toThrow("1 and 365");

    const updated = await client.updateInboxAgentConfig({
      agentId: "stale_reads",
      enabled: true,
      ageDays: 14,
      maxThreadsPerRun: 5,
      includeStarred: true,
      accountId: DEMO_ACCOUNT_ID,
      scheduleHours: 24,
    });
    expect(updated.enabled).toBe(true);
    expect(updated.ageDays).toBe(14);
    expect(updated.scheduleHours).toBe(24);
  });

  it("generates proposals, avoids re-proposing dismissed items, and supersedes archived threads", async () => {
    const dataset = defaultDemoDataset();
    dataset.tasks.push({
      id: "task-done",
      accountId: DEMO_ACCOUNT_ID,
      threadId: "welcome",
      sourceMessageId: null,
      subjectSnapshot: "Welcome to ThreeStrands",
      title: "Read welcome guide",
      notes: null,
      kind: "action",
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      goalId: null,
      status: "completed",
      completionSource: "user",
      evidenceText: null,
      waitAfter: null,
      completedByEmail: null,
      createdAt: "2026-03-05T17:00:00Z",
      updatedAt: "2026-03-05T17:00:00Z",
    });
    const client = createDemoClient(dataset);

    const afterSweeper = await client.runInboxAgent("resolved_and_expired");
    expect(afterSweeper.proposals).toHaveLength(1);
    expect(afterSweeper.proposals[0].kind).toBe("archive_as_resolved");
    expect(afterSweeper.proposals[0].threadId).toBe("welcome");

    // Dismissing hides it and prevents re-proposing for the same revision.
    await client.resolveInboxAgentProposal(afterSweeper.proposals[0].id, "dismissed");
    const afterDismiss = await client.runInboxAgent("resolved_and_expired");
    expect(afterDismiss.proposals).toHaveLength(0);

    // Aging replies generates a draft proposal, which supersedes if the thread is archived.
    const afterReplies = await client.runInboxAgent("aging_replies");
    expect(afterReplies.proposals.some((p) => p.kind === "draft_reply")).toBe(true);

    const firstReply = afterReplies.proposals.find((p) => p.kind === "draft_reply")!;
    await client.mutateThread({ kind: "archive", threadId: firstReply.threadId, value: true });
    const afterArchive = await client.getInboxAgentsSnapshot();
    expect(afterArchive.proposals.some((p) => p.id === firstReply.id)).toBe(false);
  });
});
