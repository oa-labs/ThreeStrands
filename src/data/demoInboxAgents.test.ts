import { describe, expect, it, vi } from "vitest";
import { createDemoClient } from "./demoClient";
import { defaultDemoDataset, DEMO_ACCOUNT_ID } from "./demoDataset";

describe("demo inbox assistants", () => {
  it("lists all five assistants disabled by default and validates config updates", async () => {
    const client = createDemoClient(defaultDemoDataset());
    const initial = await client.getInboxAgentsSnapshot();
    expect(initial.runnerEnabled).toBe(true);
    expect(initial.agents).toHaveLength(5);
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
    expect(afterSweeper.proposals[0].payload.taskCompletionSource).toBe("user");

    // Dismissing hides it and prevents re-proposing for the same revision.
    await client.resolveInboxAgentProposal(afterSweeper.proposals[0].id, "dismissed");
    const afterDismiss = await client.runInboxAgent("resolved_and_expired");
    expect(afterDismiss.proposals).toHaveLength(0);

    // Aging replies generates a draft proposal, which supersedes if the thread is archived.
    const afterReplies = await client.runInboxAgent("aging_replies");
    expect(afterReplies.proposals.some((p) => p.kind === "draft_reply")).toBe(true);

    // Stale reads emits structured takeaway bullets.
    const afterStale = await client.runInboxAgent("stale_reads");
    const staleProposal = afterStale.proposals.find((p) => p.kind === "archive_with_summary");
    expect(staleProposal?.payload.takeaways?.length).toBeGreaterThan(0);

    const firstReply = afterReplies.proposals.find((p) => p.kind === "draft_reply")!;
    await client.mutateThread({ kind: "archive", threadId: firstReply.threadId, value: true });
    const afterArchive = await client.getInboxAgentsSnapshot();
    expect(afterArchive.proposals.some((p) => p.id === firstReply.id)).toBe(false);
  });

  it("sweeper ignores reply-closed, email-closed, and active-task threads", async () => {
    const dataset = defaultDemoDataset();
    dataset.tasks.push(
      {
        id: "task-email-closed",
        accountId: DEMO_ACCOUNT_ID,
        threadId: "welcome",
        sourceMessageId: null,
        subjectSnapshot: "Welcome to ThreeStrands",
        title: "Email closed follow-up",
        notes: null,
        kind: "follow_up",
        dueKind: "none",
        dueValue: null,
        timeZone: null,
        repeatIntervalDays: null,
        goalId: null,
        status: "completed",
        completionSource: "user",
        evidenceText: null,
        waitAfter: null,
        completedByEmail: {
          accountId: DEMO_ACCOUNT_ID,
          subject: "Re: Welcome",
          sentAt: 1772730000,
          messageId: null,
          threadId: "welcome",
          recipients: [],
        },
        createdAt: "2026-03-05T17:00:00Z",
        updatedAt: "2026-03-05T17:00:00Z",
        completedAt: "2026-03-05T17:00:00Z",
      },
      {
        id: "task-reply-closed",
        accountId: DEMO_ACCOUNT_ID,
        threadId: "welcome",
        sourceMessageId: null,
        subjectSnapshot: "Welcome to ThreeStrands",
        title: "Reply closed wait",
        notes: null,
        kind: "waiting_for",
        dueKind: "none",
        dueValue: null,
        timeZone: null,
        repeatIntervalDays: null,
        goalId: null,
        status: "completed",
        completionSource: "reply",
        evidenceText: null,
        waitAfter: null,
        completedByEmail: null,
        createdAt: "2026-03-05T17:00:00Z",
        updatedAt: "2026-03-05T17:00:00Z",
        completedAt: "2026-03-05T17:00:00Z",
      },
    );
    const client = createDemoClient(dataset);
    const swept = await client.runInboxAgent("resolved_and_expired");
    expect(swept.proposals).toHaveLength(0);
  });

  it("sender hygiene proposes a sender with several Inbox conversations, leading with its unsubscribe message", async () => {
    const dataset = defaultDemoDataset();
    const base = dataset.threads[0];
    for (const [id, minutes] of [["promo-1", 10], ["promo-2", 20]] as const) {
      dataset.threads.push({
        ...base,
        id,
        subject: `Deals ${id}`,
        participants: ["Acme Deals"],
        lastMessageAt: `2026-03-04T10:${minutes}:00Z`,
        lastReceivedAt: `2026-03-04T10:${minutes}:00Z`,
        unread: true,
        starred: false,
        archived: false,
        trashed: false,
        labels: ["INBOX"],
      });
      dataset.messages[id] = [{
        sender: "Acme Deals <deals@acme.example>",
        recipients: ["You <you@example.com>"],
        sentAt: `2026-03-04T10:${minutes}:00Z`,
        bodyHtml: "<p>Sale</p>",
        bodyText: "Sale",
        unsubscribe: { methods: ["oneClick"], listId: "acme.example" },
      }];
    }
    const client = createDemoClient(dataset);
    const snapshot = await client.runInboxAgent("sender_hygiene");
    const proposals = snapshot.proposals.filter((p) => p.kind === "sender_hygiene" && p.payload.senderEmail === "deals@acme.example");
    expect(proposals).toHaveLength(1);
    const [proposal] = proposals;
    expect(proposal.agentId).toBe("sender_hygiene");
    expect(proposal.payload.senderEmail).toBe("deals@acme.example");
    expect(proposal.payload.recommendation).toBe("unsubscribe");
    expect(proposal.payload.messageId).toBe(`${proposal.threadId}-message`);
    expect([...(proposal.payload.inboxThreadIds ?? [])].sort()).toEqual(["promo-1", "promo-2"]);

    // Running again does not duplicate a pending suggestion.
    const again = await client.runInboxAgent("sender_hygiene");
    expect(again.proposals.filter((p) => p.payload.senderEmail === "deals@acme.example")).toHaveLength(1);
  });

  it("stale reads digest rolls unread bulk reads into per-thread digest items and never doubles up with stale reads", async () => {
    const dataset = defaultDemoDataset();
    const base = dataset.threads[0];
    for (const [id, day] of [["letter-1", "01"], ["letter-2", "02"]] as const) {
      dataset.threads.push({
        ...base,
        id,
        subject: `Weekend Reader ${id}`,
        snippet: `Issue ${id}: a long read about slow software.`,
        participants: ["Weekend Reader"],
        lastMessageAt: `2026-02-${day}T10:00:00Z`,
        lastReceivedAt: `2026-02-${day}T10:00:00Z`,
        unread: true,
        labels: ["INBOX", "CATEGORY_UPDATES"],
      });
      dataset.messages[id] = [{
        sender: "Weekend Reader <hi@reader.example>",
        recipients: ["You <you@example.com>"],
        sentAt: `2026-02-${day}T10:00:00Z`,
        bodyHtml: "<p>Read</p>",
        bodyText: "Read",
      }];
    }
    // Read already, so it never joins the digest.
    dataset.threads.push({ ...base, id: "read-letter", unread: false, labels: ["INBOX", "CATEGORY_UPDATES"], lastReceivedAt: "2026-02-03T10:00:00Z", lastMessageAt: "2026-02-03T10:00:00Z" });
    dataset.messages["read-letter"] = dataset.messages.welcome;
    const client = createDemoClient(dataset);

    const initial = await client.getInboxAgentsSnapshot();
    const digest = initial.agents.find((a) => a.agentId === "stale_reads_digest")!;
    expect(digest.scheduleHours).toBe(168);
    expect(digest.enabled).toBe(false);

    const snapshot = await client.runInboxAgent("stale_reads_digest");
    const items = snapshot.proposals.filter((p) => p.kind === "digest_item");
    expect(items.map((p) => p.threadId).sort()).toEqual(["letter-1", "letter-2", "welcome"]);
    expect(items.every((p) => p.agentId === "stale_reads_digest")).toBe(true);
    const letter = items.find((p) => p.threadId === "letter-1")!;
    expect(letter.payload.summary).toBe("Issue letter-1: a long read about slow software.");
    expect(letter.payload.takeaways).toEqual([letter.payload.summary]);
    expect(letter.payload.suggestSaveToNote).toBe(false);
    expect(letter.reason).toMatch(/^Unread for \d+ days$/);

    // A second run does not duplicate pending items, and Stale Reads skips digested threads.
    const again = await client.runInboxAgent("stale_reads_digest");
    expect(again.proposals.filter((p) => p.kind === "digest_item")).toHaveLength(3);
    const stale = await client.runInboxAgent("stale_reads");
    const digested = new Set(items.map((p) => p.threadId));
    expect(stale.proposals.filter((p) => p.kind === "archive_with_summary" && digested.has(p.threadId))).toHaveLength(0);

    // Dismissing one item keeps it out of the next run.
    await client.resolveInboxAgentProposal(letter.id, "dismissed");
    const afterDismiss = await client.runInboxAgent("stale_reads_digest");
    expect(afterDismiss.proposals.some((p) => p.threadId === "letter-1")).toBe(false);
  });

  it("expires unanswered suggestions at their kind's limit and does not suggest them again", async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date("2026-11-01T12:00:00Z"));
      const client = createDemoClient(defaultDemoDataset());
      const first = await client.runInboxAgent("aging_replies");
      const reply = first.proposals.find((p) => p.kind === "draft_reply")!;
      const stale = await client.runInboxAgent("stale_reads");
      const summary = stale.proposals.find((p) => p.kind === "archive_with_summary")!;

      // Suggested Reply: 7 days. Stale Read: 14 days.
      vi.setSystemTime(new Date("2026-11-08T11:59:59Z"));
      expect((await client.getInboxAgentsSnapshot()).proposals.some((p) => p.id === reply.id)).toBe(true);
      vi.setSystemTime(new Date("2026-11-08T12:00:00Z"));
      const week = await client.getInboxAgentsSnapshot();
      expect(week.proposals.some((p) => p.id === reply.id)).toBe(false);
      expect(week.proposals.some((p) => p.id === summary.id)).toBe(true);
      vi.setSystemTime(new Date("2026-11-15T12:00:00Z"));
      expect((await client.getInboxAgentsSnapshot()).proposals.some((p) => p.id === summary.id)).toBe(false);

      // Expired items stay out of later runs for the same revision.
      const again = await client.runInboxAgent("aging_replies");
      expect(again.proposals.some((p) => p.threadId === reply.threadId && p.kind === "draft_reply")).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });
});
