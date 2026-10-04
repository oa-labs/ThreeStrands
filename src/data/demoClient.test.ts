import { afterEach, describe, expect, it } from "vitest";
import { createDemoClient, DEMO_ACCOUNT_ID, demoClient } from "./demoClient";
import { defaultDemoDataset } from "./demoDataset";
import { buildShowcaseDataset, SHOWCASE_WORK_ACCOUNT } from "./showcaseDataset";

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

    // "welcome" (unread, participant "ThreeStrands") is the only seeded thread this rule matches.
    const split = await demoClient.createSplitInbox("ThreeStrands", "pattern", "threestrands", DEMO_ACCOUNT_ID);
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
    await demoClient.createSplitInbox("ThreeStrands", "pattern", "threestrands", OTHER_ACCOUNT_ID);

    const after = await demoClient.listThreads();
    expect(after.map((thread) => thread.id)).toEqual(before.map((thread) => thread.id));

    const afterCounts = await demoClient.mailboxUnreadCounts();
    expect(afterCounts.inbox).toBe(beforeCounts.inbox);
    expect(afterCounts.splits[productSplit.id]).toBeUndefined();

    const page = await demoClient.listSplitInboxPage(productSplit.id, 0, 10);
    expect(page.threads).toEqual([]);
  });
});

describe("demoClient default dataset messages", () => {
  it("lists the contact address on recent mail instead of the mailbox address", async () => {
    const dataset = defaultDemoDataset();
    dataset.contacts.push({ email: "hello@threestrands.local", displayName: "ThreeStrands", sentCount: 0, receivedCount: 1, lastInteractedAt: "2026-03-05T16:30:00Z", pinned: false });
    const client = createDemoClient(dataset);
    const timeline = await client.contactTimeline("derived:hello@threestrands.local");
    expect(timeline).toContainEqual(expect.objectContaining({
      threadId: "welcome",
      accountId: DEMO_ACCOUNT_ID,
      contactEmail: "hello@threestrands.local",
    }));
  });

  it("keeps unsubscribe limited to messages that advertise it", async () => {
    const client = createDemoClient(defaultDemoDataset());
    await expect(client.unsubscribe("welcome-message")).resolves.toMatchObject({ method: "oneClick", outcome: "requested" });
    await expect(client.unsubscribe("roadmap-message")).rejects.toThrow("no unsubscribe option");
  });
});

describe("showcase dataset", () => {
  const now = new Date("2026-09-22T17:24:00Z");
  const RESERVED_DOMAIN = /(^|\.)(example|test|invalid)$|^example\.(com|net|org)$/;

  it("only contains addresses and links on reserved domains", () => {
    const serialized = JSON.stringify(buildShowcaseDataset(now));
    const domains = [
      ...[...serialized.matchAll(/[\w.+-]+@([\w-]+(?:\.[\w-]+)+)/g)].map((match) => match[1]!),
      ...[...serialized.matchAll(/https?:\/\/([\w-]+(?:\.[\w-]+)+)/g)].map((match) => match[1]!),
    ];
    expect(domains.length).toBeGreaterThan(20);
    expect(domains.filter((domain) => !RESERVED_DOMAIN.test(domain.toLocaleLowerCase()))).toEqual([]);
  });

  it("produces reproducible AI fixtures with evidence from the fictional mailbox", async () => {
    const dataset = buildShowcaseDataset(now);
    expect(dataset.aiFixtures).toEqual(buildShowcaseDataset(now).aiFixtures);
    const client = createDemoClient(dataset);
    const tasksBefore = await client.listTasks();
    const brief = await client.briefThread("renewal", "UTC", "openai", "fictional-model", null);
    expect(brief.summary.summary).toContain("pricing for 35 seats");
    expect(brief.analysis.proposals).toHaveLength(1);
    const detail = await client.getThread("renewal");
    for (const proposal of brief.analysis.proposals) {
      const source = detail.messages.find((message) => message.id === proposal.evidence.sourceMessageId);
      expect(source?.bodyText).toContain(proposal.evidence.excerpt);
    }
    expect(detail.thread.summary).toBe(brief.summary.summary);
    expect(await client.listTasks()).toEqual(tasksBefore);
    brief.analysis.proposals[0]!.title = "An edit to the returned copy";
    const again = await client.analyzeThread("renewal", "UTC", "openai", "fictional-model", null);
    expect(again.proposals[0]?.title).toBe("Send Marcus pricing for 35 seats");
  });

  it("keeps a handled suggestion out of saved suggestions until a brief asks again", async () => {
    const dataset = defaultDemoDataset();
    const client = createDemoClient(dataset);
    const thread = dataset.threads[0]!;
    const [suggestion] = (await client.analyzeThread(thread.id, "UTC", "openai", "fictional-model", null)).proposals;

    expect(await client.removeThreadSuggestion(thread.id, "2000-01-01T00:00:00Z", suggestion!)).toBe(false);
    expect(await client.removeThreadSuggestion(thread.id, thread.lastMessageAt, { ...suggestion!, title: "Never saved" })).toBe(false);
    expect(await client.removeThreadSuggestion(thread.id, thread.lastMessageAt, suggestion!)).toBe(true);
    expect((await client.analyzeThread(thread.id, "UTC", "openai", "fictional-model", null)).proposals).toEqual([]);

    // A brief always asks again and replaces what was saved.
    const brief = await client.briefThread(thread.id, "UTC", "openai", "fictional-model", null);
    expect(brief.analysis.proposals).toHaveLength(1);
    expect((await client.analyzeThread(thread.id, "UTC", "openai", "fictional-model", null)).proposals).toHaveLength(1);
  });

  it("shares only selected attachments and returns other-mail sources only for a requested search", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const request = {
      threadId: "launch-plan", question: "What changed in onboarding and how will we roll it out?",
      history: [], searchMailbox: true, includeProposals: false, contactId: null, userTimeZone: "UTC",
      attachments: [{ messageId: "launch-plan-message", attachmentId: "launch-plan-pdf" }],
    };
    const searched = await client.threadChat(request, "openai", "fictional-model", null);
    expect(searched.answer).toContain("EU and APAC following 48 hours later");
    expect(searched.attachments).toEqual([{ ...request.attachments[0], filename: "Q4-launch-plan-v7.pdf", truncated: false }]);
    expect(searched.sources).toHaveLength(1);
    expect(searched.searched).toEqual(searched.sources);
    const source = await client.getThread(searched.sources[0]!.threadId);
    expect(source.thread.subject).toBe(searched.sources[0]?.subject);
    expect(source.messages[0]?.bodyText).toContain("after the first inbox connects");
    const local = await client.threadChat({ ...request, searchMailbox: false, attachments: [] }, "openai", "fictional-model", null);
    expect(local.sources).toEqual([]);
    expect(local.searched).toEqual([]);
    expect(local.attachments).toEqual([]);
    expect(local.answer).not.toContain("asks for the team name");
  });

  it("keeps the default demo's generic AI behavior and opt-in fixtures independent", async () => {
    const dataset = defaultDemoDataset();
    expect(dataset.aiFixtures).toBeUndefined();
    const client = createDemoClient(dataset);
    const thread = dataset.threads[0]!;
    const analysis = await client.analyzeThread(thread.id, "UTC", "openai", "fictional-model", null);
    expect(analysis.proposals[0]?.title).toBe(`Review: ${thread.subject}`);
    const summary = await client.summarizeThread(thread.id, "openai", "fictional-model", null);
    expect(summary.summary).toContain(thread.subject);
    expect(summary.summary).not.toContain("Brightwater");
  });

  it("dates mail relative to now and schedules the current week", () => {
    const dataset = buildShowcaseDataset(now);
    const newest = Math.max(...dataset.threads.map((thread) => Date.parse(thread.lastMessageAt)));
    expect(newest).toBeLessThanOrEqual(now.getTime());
    expect(now.getTime() - newest).toBeLessThan(60 * 60_000);
    const timed = dataset.scheduleEvents.filter((event) => !event.allDay);
    expect(timed.some((event) => new Date(event.start).toDateString() === now.toDateString())).toBe(true);
  });

  it("serves multi-message threads whose later messages resolve for replies", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const detail = await client.getThread("launch-plan");
    expect(detail.messages.map((message) => message.id)).toEqual(["launch-plan-message", "launch-plan-message-2", "launch-plan-message-3"]);
    expect(detail.messages.filter((message) => message.unread)).toHaveLength(1);
    expect(detail.thread.participants).toContain(`Maya Chen <${SHOWCASE_WORK_ACCOUNT}>`);

    const draft = await client.createDraft("reply", "launch-plan-message-3");
    expect(draft.account).toBe(SHOWCASE_WORK_ACCOUNT);
    expect(draft.to).toContain("priya@harborlight.example");
    const context = await client.replyAssistContext(draft.id);
    expect(context.messages).toHaveLength(3);
    await client.discardDraft(draft.id);
  });

  it("claims notification mail for its split inbox and keeps it out of the Inbox tab", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const inbox = await client.listThreads(SHOWCASE_WORK_ACCOUNT);
    expect(inbox.some((thread) => thread.id === "ci-pr")).toBe(false);
    const page = await client.listSplitInboxPage("split-notifications", 0, 10);
    expect(page.threads.map((thread) => thread.id)).toEqual(expect.arrayContaining(["ci-pr", "ci-deploy"]));
  });

  it("returns only schedule events overlapping the requested range, including all-day events", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const dayStart = new Date(now);
    dayStart.setHours(0, 0, 0, 0);
    const dayEnd = new Date(dayStart);
    dayEnd.setDate(dayEnd.getDate() + 1);
    const { events } = await client.listScheduleEvents(dayStart.toISOString(), dayEnd.toISOString(), "UTC");
    expect(events.length).toBeGreaterThan(0);
    expect(events.every((event) => event.allDay || (Date.parse(event.end) > dayStart.getTime() && Date.parse(event.start) < dayEnd.getTime()))).toBe(true);
    expect(events.some((event) => event.id === "offsite")).toBe(false);

    const nextWeek = new Date(dayStart);
    nextWeek.setDate(nextWeek.getDate() + 14);
    const wide = await client.listScheduleEvents(dayStart.toISOString(), nextWeek.toISOString(), "UTC");
    expect(wide.events.some((event) => event.id === "offsite")).toBe(true);
  });

  it("creates a meeting on a writable calendar and includes it in the schedule", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const calendar = (await client.listCalendarOptions()).find((option) => option.accountId === SHOWCASE_WORK_ACCOUNT && option.primary)!;
    const start = new Date(now.getTime() + 60 * 60_000).toISOString();
    const end = new Date(now.getTime() + 2 * 60 * 60_000).toISOString();
    const created = await client.createCalendarEvent({
      accountId: SHOWCASE_WORK_ACCOUNT, calendarId: calendar.id, title: "Planning",
      start, end, description: "Agenda", attendees: ["guest@example.com"],
    });
    expect(created).toMatchObject({ title: "Planning", start, end, description: "Agenda", attendees: ["guest@example.com"] });
    const schedule = await client.listScheduleEvents(start, end, "UTC");
    expect(schedule.events).toContainEqual(created);
    await expect(client.createCalendarEvent({
      accountId: SHOWCASE_WORK_ACCOUNT, calendarId: "unknown", title: "Nope",
      start, end, description: "", attendees: [],
    })).rejects.toThrow("Choose a calendar");
  });

  it("updates the response to a demo invitation and rejects events without an RSVP", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    const start = new Date(now.getTime() - 7 * 86_400_000).toISOString();
    const end = new Date(now.getTime() + 14 * 86_400_000).toISOString();
    const schedule = await client.listScheduleEvents(start, end, "UTC");
    const invitation = schedule.events.find((event) => event.id === "crit")!;
    expect(invitation.responseStatus).toBe("needsAction");
    expect((await client.updateCalendarResponse(invitation, "declined")).responseStatus).toBe("declined");
    expect((await client.listScheduleEvents(start, end, "UTC")).events.find((event) => event.id === "crit")?.responseStatus).toBe("declined");
    await expect(client.updateCalendarResponse(schedule.events.find((event) => event.id === "focus-1")!, "accepted"))
      .rejects.toThrow("no RSVP");
  });

  it("unsubscribes through the first advertised method", async () => {
    const client = createDemoClient(buildShowcaseDataset(now));
    await expect(client.unsubscribe("reader-message")).resolves.toMatchObject({ method: "oneClick" });
    await expect(client.unsubscribe("ci-pr-message")).resolves.toEqual({ method: "web", outcome: "opened", httpStatus: null });
    await expect(client.unsubscribe("dinner-message")).rejects.toThrow("no unsubscribe option");
  });
});
