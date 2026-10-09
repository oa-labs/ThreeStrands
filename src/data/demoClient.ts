import { demoCorrespondence } from "./demoCorrespondence";
import { isActiveTaskStatus } from "../taskViews";
import { goalPeriodMatches, parentCandidates } from "../goals";
import type { MailClient } from "./client";
import { DEMO_ACCOUNT_ID, defaultDemoDataset, type DemoDataset } from "./demoDataset";
import { buildShowcaseDataset } from "./showcaseDataset";
import { parseAddress, splitAddressList } from "../emailAddress";
import { isCalendarAttachment } from "../CalendarAttachment";
import { MAX_KEEP_IN_TOUCH_DAYS } from "../keepInTouch";
import { MAX_CONTACT_GROUP_MEMBERS, MAX_CONTACT_GROUP_NAME, MAX_CONTACT_GROUPS } from "../contactGroups";
import type {
  ContactGroup,
  Account,
  ActionAnalysis,
  AiUsageDay,
  AvailabilityPreferences,
  AvailabilityResult,
  CalendarAccount,
  ContactActivity,
  ContactFile,
  ContactProfile,
  ContactTimelineItem,
  KeepInTouch,
  DomainPerson,
  SaveContactRequest,
  CreateTaskRequest,
  CreateGoalRequest,
  Goal,
  Label,
  Message,
  ReplyAssistContext,
  ReplyAssistResult,
  DraftReviewResult,
  ScheduleEvent,
  Snippet,
  SplitInbox,
  SummaryResult,
  ThreadBriefResult,
  ThreadChatReply,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadTask,
  ProposedTimeCheck,
  ThreadPage,
  ThreadMutation,
  TriageEvent,
  TriageSenderStats,
  UnsubscribeResult,
  UpdateTaskRequest,
  UpdateGoalRequest,
} from "../domain";

export { DEMO_ACCOUNT_ID };

/** Builds an in-memory browser-preview client that starts from its own copy of `dataset`. */
export function createDemoClient(dataset: DemoDataset): MailClient {
  const seed = structuredClone(dataset);
  let accounts = seed.accounts;
  let calendarAccounts = seed.calendarAccounts;
  let calendarOptions = seed.calendarOptions;
  let threads = seed.threads;
  let labels = seed.labels;
  let splitInboxes = seed.splitInboxes;
  let tasks = seed.tasks;
  let goals = seed.goals ?? [];
  const ensureGoalLink = (accountId: string, goalId: string | null | undefined) => {
    if (!goalId) return;
    const goal = goals.find((candidate) => candidate.id === goalId);
    if (!goal) throw new Error("Goal not found");
    if (goal.accountId !== accountId) throw new Error("A task can only support a goal in its own account");
  };
  const validateGoal = (goal: Goal) => {
    if (!goal.title.trim() || goal.title.trim().length > 240) throw new Error("Goal title must be between 1 and 240 characters");
    if (!goalPeriodMatches(goal.horizon, goal.period)) throw new Error("Choose a period that matches the goal's horizon, such as 2026, 2026-H2, or 2026-Q4");
    if (goal.parentGoalId && !parentCandidates(goals, goal).some((candidate) => candidate.id === goal.parentGoalId)) {
      throw new Error("A goal can only support a longer-term goal whose period includes it");
    }
  };
  let snippets = seed.snippets;
  let contacts = seed.contacts;
  const suppressedContacts = new Set<string>();
  /** A saved profile for `id`, saving a `derived:<email>` contact first. */
  const ensureSavedContact = async (id: string): Promise<ContactProfile> => {
    const saved = savedContactProfiles.find((item) => item.id === id);
    if (saved) return saved;
    const derived = await client.getContactProfile(id);
    if (!derived) throw new Error("Contact not found");
    return client.saveContactProfile({ ...derived, id: derived.id });
  };
  let contactGroups: ContactGroup[] = seed.contactGroups ?? [];
  /** Mirrors the backend: members are listed by name and only while their contact is saved. */
  const presentGroup = (group: ContactGroup): ContactGroup => {
    const name = (id: string) => savedContactProfiles.find((profile) => profile.id === id)?.displayName ?? null;
    const memberIds = group.memberIds
      .filter((id) => savedContactProfiles.some((profile) => profile.id === id))
      .sort((a, b) => (name(a) === null ? 1 : 0) - (name(b) === null ? 1 : 0) || (name(a) ?? "").localeCompare(name(b) ?? "", undefined, { sensitivity: "base" }) || a.localeCompare(b));
    return structuredClone({ ...group, memberIds });
  };
  const validGroupName = (name: string, exceptId?: string) => {
    const trimmed = name.trim();
    if (!trimmed || [...trimmed].length > MAX_CONTACT_GROUP_NAME || /\p{Cc}/u.test(trimmed)) throw new Error(`Group names must be between 1 and ${MAX_CONTACT_GROUP_NAME} characters`);
    const existing = contactGroups.find((group) => group.id !== exceptId && group.name.toLocaleLowerCase() === trimmed.toLocaleLowerCase());
    if (existing) throw new Error(`A group named \u201c${existing.name}\u201d already exists`);
    return trimmed;
  };
  const resolveGroupMembers = async (contactIds: string[], emails: string[]) => {
    const resolved: string[] = [];
    for (const id of contactIds) resolved.push((await ensureSavedContact(id)).id);
    for (const raw of emails) {
      const email = raw.trim().toLocaleLowerCase();
      if (!email.includes("@") || /\s/.test(email)) throw new Error(`\u201c${raw.trim()}\u201d isn't a valid email address`);
      const owner = savedContactProfiles.find((profile) => profile.addresses.includes(email));
      resolved.push(owner ? owner.id : (await client.saveContactProfile({ id: null, displayName: null, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: false, addresses: [email], birthday: null })).id);
    }
    return resolved;
  };
  let savedContactProfiles: ContactProfile[] = seed.contactProfiles.map((profile) => withKeepInTouchDue({ birthday: null, keepInTouch: { ...NO_KEEP_IN_TOUCH }, keepInTouchDueAt: null, ...profile }));
  const { details, messages: seededMessages } = seed;
  const scheduleEvents = seed.scheduleEvents;

  const status: SyncStatus = {
    state: "idle",
    lastSuccessfulSync: null,
    cursor: "demo",
    pendingMutations: 0,
    failedMutations: [],
    quarantinedMessages: [],
    error: null,
  };

  function visibleWhere(accountId: string | undefined, predicate: (thread: Thread) => boolean): Thread[] {
    const connected = new Set(accounts.map((account) => account.email));
    return threads
      .filter((thread) => connected.has(thread.accountId))
      .filter(predicate)
      .filter((thread) => !accountId || accountId === "all" || thread.accountId === accountId)
      .sort((a, b) => b.lastReceivedAt.localeCompare(a.lastReceivedAt));
  }

  function visible(accountId?: string): Thread[] {
    return visibleWhere(accountId, (thread) => !thread.archived && !thread.trashed);
  }

  /** Gmail's "All Mail": everything except Trash. */
  function visibleAllMail(accountId?: string): Thread[] {
    return visibleWhere(accountId, (thread) => !thread.trashed);
  }

  function visibleTrash(accountId?: string): Thread[] {
    return visibleWhere(accountId, (thread) => thread.trashed);
  }

  /** Mirrors `split_inbox_matches` in `src-tauri/src/db.rs` for the browser-preview build. */
  function matchesSplitInbox(rule: SplitInbox, thread: Thread): boolean {
    switch (rule.matchKind) {
      case "domain":
        return thread.participants.some(
          (participant) => parseAddress(participant).email.split("@")[1]?.toLocaleLowerCase() === rule.matchValue,
        );
      case "label":
        return thread.labels.includes(rule.matchValue);
      case "pattern":
        return thread.participants.some((participant) =>
          parseAddress(participant).email.toLocaleLowerCase().includes(rule.matchValue),
        );
    }
  }

  /** Always scoped to the rule's own account — a split inbox belongs to one account. */
  function visibleSplitInbox(splitInboxId: string): Thread[] {
    const rule = splitInboxes.find((candidate) => candidate.id === splitInboxId);
    if (!rule) throw new Error("Split inbox not found");
    return visible(rule.accountId).filter((thread) => matchesSplitInbox(rule, thread));
  }

  /**
   * The Inbox tab is `visible()` minus anything a split inbox rule claims —
   * a split inbox pulls its matches out of the Inbox rather than mirroring
   * them into a second view. A rule only ever excludes threads from its own
   * account. Mirrors `list_threads_page` in `src-tauri/src/db.rs`.
   */
  function visibleInbox(accountId?: string): Thread[] {
    if (splitInboxes.length === 0) return visible(accountId);
    return visible(accountId).filter(
      (thread) => !splitInboxes.some((rule) => rule.accountId === thread.accountId && matchesSplitInbox(rule, thread)),
    );
  }

  /**
   * Splits a query into quoted phrases and standalone words, mirroring the
   * FTS5 query builder in `src-tauri/src/db.rs` well enough for the demo/test
   * build: `query.split('"')` alternates unquoted segments (even indices) with
   * quoted ones (odd indices), and an unterminated trailing quote is treated
   * as still-quoted.
   */
  function parseQueryParts(query: string): { phrases: string[]; words: string[] } {
    const phrases: string[] = [];
    const words: string[] = [];
    query.split('"').forEach((segment, index) => {
      const normalized = segment.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
      if (index % 2 === 0) {
        words.push(...normalized);
      } else if (normalized.length > 0) {
        phrases.push(normalized.join(" "));
      }
    });
    return { phrases, words };
  }

  function matchesQuery(haystack: string, query: string): boolean {
    const normalizedHaystack = haystack.toLocaleLowerCase();
    const { phrases, words } = parseQueryParts(query);
    return (
      phrases.every((phrase) => normalizedHaystack.includes(phrase)) &&
      words.every((word) => normalizedHaystack.includes(word))
    );
  }

  function update(mutation: ThreadMutation) {
    threads = threads.map((thread) => {
      if (thread.id !== mutation.threadId) return thread;
      switch (mutation.kind) {
        case "archive":
          return { ...thread, archived: mutation.value };
        case "trash":
          return { ...thread, trashed: mutation.value };
        case "spam": {
          const next = new Set(thread.labels);
          if (mutation.value) {
            next.add("SPAM");
            next.delete("INBOX");
          } else {
            next.delete("SPAM");
            next.add("INBOX");
          }
          return { ...thread, archived: mutation.value, labels: [...next] };
        }
        case "read":
          return { ...thread, unread: !mutation.value };
        case "star":
          return { ...thread, starred: mutation.value };
        case "label": {
          const next = new Set(thread.labels);
          if (mutation.value) next.add(mutation.labelId);
          else next.delete(mutation.labelId);
          return { ...thread, labels: [...next] };
        }
      }
    });
  }

  /**
   * The first message keeps the `${threadId}-message` id the single-message
   * threads have always used; later ones get a position suffix.
   */
  function messagesFor(thread: Thread): Message[] {
    const seeds = seededMessages[thread.id];
    if (!seeds) {
      return [{
        id: `${thread.id}-message`,
        threadId: thread.id,
        sender: `${thread.participants[0]} <hello@threestrands.local>`,
        recipients: ["You <you@example.com>"],
        sentAt: thread.lastMessageAt,
        bodyHtml: details[thread.id] ?? `<p>${thread.snippet}</p>`,
        bodyText: thread.snippet,
        unread: thread.unread,
        unsubscribe: null,
        attachments: [],
      }];
    }
    return seeds.map((message, index) => ({
      id: index === 0 ? `${thread.id}-message` : `${thread.id}-message-${index + 1}`,
      threadId: thread.id,
      sender: message.sender,
      recipients: [...message.recipients],
      sentAt: message.sentAt,
      bodyHtml: message.bodyHtml,
      bodyText: message.bodyText,
      unread: thread.unread && index === seeds.length - 1,
      unsubscribe: message.unsubscribe ?? null,
      attachments: structuredClone(message.attachments ?? []),
    }));
  }

  function findMessage(messageId: string): Message | undefined {
    for (const thread of threads) {
      const message = messagesFor(thread).find((candidate) => candidate.id === messageId);
      if (message) return message;
    }
    return undefined;
  }

  function threadForMessage(messageId: string): Promise<ThreadDetail> {
    const message = findMessage(messageId);
    if (!message) return Promise.reject(new Error("Message not found"));
    return client.getThread(message.threadId);
  }

  /** All-day events carry bare `YYYY-MM-DD` dates (end exclusive), as Google Calendar returns them. */
  function eventRange(event: ScheduleEvent): [number, number] {
    if (!event.allDay) return [Date.parse(event.start), Date.parse(event.end)];
    const localMidnight = (value: string) => {
      const [year, month, day] = value.split("-").map(Number);
      return new Date(year, month - 1, day).getTime();
    };
    return [localMidnight(event.start), localMidnight(event.end)];
  }

  /** Timed events on connected calendars, as busy intervals; all-day events do not block time. */
  function demoBusyIntervals(): [number, number][] {
    const connected = new Set(calendarAccounts.filter((account) => account.status === "connected").map((account) => account.email));
    return scheduleEvents
      .filter((event) => !event.allDay && connected.has(event.accountId))
      .map(eventRange);
  }

  /** Thread id to the suggestions saved for its newest message, as the native client keeps them. */
  const savedAnalyses = new Map<string, { revision: string; analysis: ActionAnalysis }>();
  const saveAnalysis = (threadId: string, revision: string, analysis: ActionAnalysis) => {
    savedAnalyses.set(threadId, { revision, analysis: structuredClone(analysis) });
    return analysis;
  };
  const generateAnalysis = async (threadId: string): Promise<ActionAnalysis> => {
    const detail = await client.getThread(threadId);
    const latest = detail.messages.at(-1);
    if (!latest) return { proposals: [], hiddenCount: 0 };
    await new Promise((resolve) => setTimeout(resolve, 400));
    const fixture = seed.aiFixtures?.[threadId]?.analysis;
    if (fixture) return structuredClone(fixture);
    return { hiddenCount: 0, proposals: [{
      type: "task",
      kind: "action",
      title: `Review: ${detail.thread.subject}`,
      notes: detail.thread.snippet,
      dueKind: "none",
      dueValue: null,
      timeZone: null,
      repeatIntervalDays: null,
      confidence: 0.72,
      evidence: {
        sourceMessageId: latest.id,
        excerpt: latest.bodyText.slice(0, 240),
      },
    }] };
  };

  const client: MailClient = {
    ...demoCorrespondence(threadForMessage, () => accounts[0]?.email ?? DEMO_ACCOUNT_ID),
    async listThreads(accountId) {
      return structuredClone(visibleInbox(accountId));
    },
    async listAllMail(accountId) {
      return structuredClone(visibleAllMail(accountId));
    },
    async listTrash(accountId) {
      return structuredClone(visibleTrash(accountId));
    },
    async listThreadsPage(accountId, offset, limit): Promise<ThreadPage> {
      const items = visibleInbox(accountId);
      return { threads: structuredClone(items.slice(offset, offset + limit)), hasMore: offset + limit < items.length };
    },
    async listAllMailPage(accountId, offset, limit): Promise<ThreadPage> {
      const items = visibleAllMail(accountId);
      return { threads: structuredClone(items.slice(offset, offset + limit)), hasMore: offset + limit < items.length };
    },
    async listTrashPage(accountId, offset, limit): Promise<ThreadPage> {
      const items = visibleTrash(accountId);
      return { threads: structuredClone(items.slice(offset, offset + limit)), hasMore: offset + limit < items.length };
    },
    async listUnreadCounts() {
      return threads.reduce<Record<string, number>>((counts, thread) => {
        if (thread.unread && !thread.archived && !thread.trashed) {
          counts[thread.accountId] = (counts[thread.accountId] ?? 0) + 1;
        }
        return counts;
      }, {});
    },
    async mailboxUnreadCounts(accountId) {
      const splits: Record<string, number> = {};
      let inbox = 0;
      for (const thread of visible(accountId)) {
        if (!thread.unread) continue;
        const matchingRules = splitInboxes.filter(
          (rule) => rule.accountId === thread.accountId && matchesSplitInbox(rule, thread),
        );
        for (const rule of matchingRules) {
          splits[rule.id] = (splits[rule.id] ?? 0) + 1;
        }
        if (matchingRules.length === 0) inbox += 1;
      }
      return { inbox, splits };
    },
    // No native backend to proxy through in demo mode, so this fetches
    // directly from the browser — fine for local dev/preview, where there's
    // no real reader to protect from a sender's tracking/SSRF attempts.
    async fetchRemoteImage(url) {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`Failed to fetch image: HTTP ${response.status}`);
      const blob = await response.blob();
      return await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = () => reject(reader.error ?? new Error("Failed to read image"));
        reader.readAsDataURL(blob);
      });
    },
    async fetchAttachmentImage(_messageId, _attachmentId) {
      throw new Error("Embedded attachment images are unavailable in browser preview");
    },
    async previewCalendarAttachment(_messageId, _attachmentId) {
      throw new Error("Calendar previews are unavailable in browser preview");
    },
    async getThread(id) {
      const thread = threads.find((candidate) => candidate.id === id);
      if (!thread) throw new Error("Thread not found");
      const detail: ThreadDetail = {
        thread: structuredClone(thread),
        messages: messagesFor(thread),
      };
      return detail;
    },
    async openAttachment(_messageId, _attachmentId) {
      const url = URL.createObjectURL(new Blob(["ThreeStrands keyboard shortcuts\n\nj/k: move\ne: archive\ns: star\n"], { type: "text/plain" }));
      window.open(url, "_blank", "noopener,noreferrer");
      window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
    },
    async saveAttachment(_messageId, _attachmentId) {
      const url = URL.createObjectURL(new Blob(["ThreeStrands keyboard shortcuts\n\nj/k: move\ne: archive\ns: star\n"], { type: "text/plain" }));
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = "threestrands-shortcuts.txt";
      anchor.click();
      URL.revokeObjectURL(url);
    },
    async summarizeThread(threadId): Promise<SummaryResult> {
      const thread = threads.find((candidate) => candidate.id === threadId);
      if (!thread) throw new Error("Thread not found");
      await new Promise((resolve) => setTimeout(resolve, 400));
      const summary = seed.aiFixtures?.[threadId]?.summary ?? [
        `- ${thread.subject}`,
        `- Latest message from ${thread.participants[0] ?? "a participant"}`,
        `- ${thread.snippet}`,
      ].join("\n");
      const generatedAt = new Date().toISOString();
      const revision = thread.lastMessageAt;
      threads = threads.map((candidate) =>
        candidate.id === threadId ? { ...candidate, summary, summaryGeneratedAt: generatedAt, summaryRevision: revision } : candidate,
      );
      return { summary, generatedAt, revision };
    },
    async analyzeThread(threadId): Promise<ActionAnalysis> {
      // Like the native client, reuse suggestions saved for the thread's newest message.
      const detail = await this.getThread(threadId);
      const saved = savedAnalyses.get(threadId);
      if (saved && saved.revision === detail.thread.lastMessageAt) return structuredClone(saved.analysis);
      return saveAnalysis(threadId, detail.thread.lastMessageAt, await generateAnalysis(threadId));
    },
    async removeThreadSuggestion(threadId, revision, proposal): Promise<boolean> {
      const saved = savedAnalyses.get(threadId);
      if (!saved || saved.revision !== revision) return false;
      const target = JSON.stringify(proposal);
      const index = saved.analysis.proposals.findIndex((candidate) => JSON.stringify(candidate) === target);
      if (index < 0) return false;
      saved.analysis.proposals.splice(index, 1);
      return true;
    },
    async threadChat(request): Promise<ThreadChatReply> {
      const detail = await this.getThread(request.threadId);
      await new Promise((resolve) => setTimeout(resolve, 400));
      const wantsReply = /\b(draft|write|reply)\b/i.test(request.question);
      const wantsTimes = /\b(free|available|availability|when can|find a time)\b/i.test(request.question);
      const now = new Date();
      const fixture = seed.aiFixtures?.[request.threadId]?.chat;
      return {
        availability: wantsTimes
          ? { rangeStart: now.toISOString(), rangeEnd: new Date(now.getTime() + 7 * 86_400_000).toISOString(), durationMinutes: 30 }
          : null,
        answer: (request.searchMailbox ? fixture?.answer : undefined) ?? `In the demo, answers come from “${detail.thread.subject}”: ${detail.thread.snippet}`,
        analysis: request.includeProposals && fixture ? structuredClone(fixture.analysis) : { proposals: [], hiddenCount: 0 },
        replyDraft: fixture?.replyDraft ?? (wantsReply ? "Thanks for the update. I'll take a look and get back to you soon." : null),
        sources: request.searchMailbox && fixture ? structuredClone(fixture.sources) : [],
        searched: request.searchMailbox && fixture ? structuredClone(fixture.searched) : [],
        attachments: request.attachments.flatMap((reference) => {
          const attachment = detail.messages
            .find((message) => message.id === reference.messageId)
            ?.attachments.find((candidate) => candidate.id === reference.attachmentId);
          return attachment ? [{ ...reference, filename: attachment.filename, truncated: false }] : [];
        }),
      };
    },
    async aiUsageSummary(): Promise<AiUsageDay[]> {
      // The demo never calls a provider, so there is no usage to report.
      return [];
    },
    async briefThread(threadId, _userTimeZone, provider, model, endpoint): Promise<ThreadBriefResult> {
      // A brief always asks again and replaces the saved suggestions, as the native client does.
      const [summary, analysis, detail] = await Promise.all([
        this.summarizeThread(threadId, provider, model, endpoint),
        generateAnalysis(threadId),
        this.getThread(threadId),
      ]);
      return { summary, analysis: saveAnalysis(threadId, detail.thread.lastMessageAt, analysis) };
    },
    async replyAssistContext(draftId): Promise<ReplyAssistContext> {
      const replyDraft = (await this.listDrafts()).find((candidate) => candidate.id === draftId);
      if (!replyDraft || !["reply", "replyAll"].includes(replyDraft.mode) || !replyDraft.sourceId) {
        throw new Error("Reply Assist is only available for reply drafts");
      }
      const detail = await threadForMessage(replyDraft.sourceId);
      return {
        subject: detail.thread.subject,
        messages: detail.messages.map((message) => ({
          sender: message.sender,
          sentAt: message.sentAt,
          bodyText: message.bodyText,
        })),
      };
    },
    async reviewDraft(request): Promise<DraftReviewResult> {
      await new Promise((resolve) => setTimeout(resolve, 400));
      return {
        assessment: "Demo review: make the next step easy for the recipient to answer.",
        suggestions: [{ title: "Clarify the next step", field: "body", excerpt: request.body,
          reason: "A direct question gives the recipient a clear way to respond.",
          replacement: `${request.body.trim()}\n\nWould you be open to a brief conversation?` }],
        revisedSubject: request.subject,
        revisedBody: `${request.body.trim()}\n\nWould you be open to a brief conversation?`,
      };
    },
    async generateReply(context, instruction): Promise<ReplyAssistResult> {
      await new Promise((resolve) => setTimeout(resolve, 400));
      const request = instruction.trim();
      const latest = context.messages.at(-1);
      return {
        body: request
          ? `Thanks for the update. ${request}`
          : `Thanks for the update${latest ? `, ${latest.sender.split("<")[0].trim()}` : ""}. I'll follow up shortly.`,
      };
    },
    async searchThreads({ query, limit = 50, offset = 0, includeArchived = false }, accountId) {
      if (!query.trim()) return this.listThreads(accountId);
      const pool = includeArchived
        ? [...threads]
            .filter((thread) => !accountId || accountId === "all" || thread.accountId === accountId)
            .sort((a, b) => b.lastReceivedAt.localeCompare(a.lastReceivedAt))
        : visible(accountId);
      return structuredClone(
        pool
          .filter((thread) =>
            matchesQuery([thread.subject, thread.snippet, ...thread.participants].join(" "), query),
          )
          .slice(offset, offset + limit),
      );
    },
    async backfillSearchThreads(_query, _accountId) {},
    async mutateThread(mutation) {
      update(mutation);
    },
    async mutateThreads(mutations) {
      mutations.forEach(update);
    },
    async recordTriageEvent(_event: TriageEvent) {},
    async listTriageSenderStats(_accountId: string, _limit?: number): Promise<TriageSenderStats[]> {
      return [];
    },
    async listContactSuggestions(_accountId, query, limit = 8) {
      const needle = query.trim().toLocaleLowerCase();
      const candidates = new Map(contacts.map(contact => [contact.email.toLowerCase(), { ...contact }]));
      for (const profile of savedContactProfiles) {
        for (const email of profile.addresses) {
          const previous = candidates.get(email);
          candidates.set(email, { email, displayName: profile.displayName, pinned: profile.favorite,
            sentCount: previous?.sentCount ?? 0, receivedCount: previous?.receivedCount ?? 0,
            lastInteractedAt: previous?.lastInteractedAt ?? "" });
        }
      }
      const matches = [...candidates.values()].filter(
        (contact) => {
          if (suppressedContacts.has(contact.email.toLowerCase())) return false;
          const email = contact.email.toLocaleLowerCase();
          const domain = email.split("@").at(-1) ?? "";
          return (
            !needle ||
            email.startsWith(needle) ||
            domain.includes(needle) ||
            (contact.displayName?.toLocaleLowerCase().includes(needle) ?? false)
          );
        },
      );
      matches.sort(
        (a, b) =>
          Number(b.pinned) - Number(a.pinned) ||
          b.sentCount - a.sentCount ||
          b.receivedCount - a.receivedCount ||
          b.lastInteractedAt.localeCompare(a.lastInteractedAt),
      );
      return structuredClone(matches.slice(0, limit));
    },
    async listContactProfiles(query = "", limit = 500, accountId) {
      const byEmail = new Map<string, ContactProfile>();
      for (const item of contacts.filter((contact) => contact.sentCount > 0 || contact.pinned)) {
        const email = item.email.toLocaleLowerCase();
        if (savedContactProfiles.some((profile) => profile.addresses.includes(email))) continue;
        byEmail.set(email, { id: `derived:${email}`, displayName: item.displayName, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: item.pinned, addresses: [email], sentCount: item.sentCount, receivedCount: item.receivedCount, lastInteractedAt: item.lastInteractedAt, birthday: null, keepInTouch: { ...NO_KEEP_IN_TOUCH }, keepInTouchDueAt: null });
      }
      const values = [...savedContactProfiles, ...byEmail.values()].filter((profile) => {
        if (!accountId) return true;
        const matching = threads.filter((thread) => thread.participants.some((raw) => profile.addresses.includes(parseAddress(raw).email.toLocaleLowerCase())));
        return matching.length === 0 || matching.some((thread) => thread.accountId === accountId);
      });
      const needle = query.trim().toLocaleLowerCase();
      return structuredClone(values.filter((profile) => !needle || `${profile.displayName ?? ""} ${profile.addresses.join(" ")} ${profile.company ?? ""} ${profile.role ?? ""} ${profile.location ?? ""} ${profile.bio ?? ""} ${profile.notes ?? ""}`.toLocaleLowerCase().includes(needle))
        .sort((a,b) => Number(b.favorite)-Number(a.favorite) || (b.lastInteractedAt ?? "").localeCompare(a.lastInteractedAt ?? "") || (a.displayName ?? a.addresses[0]).localeCompare(b.displayName ?? b.addresses[0])).slice(0,limit));
    },
    async resolveContactIds(emails) {
      const owners: Record<string, string> = {};
      for (const raw of emails) {
        const email = raw.trim().toLocaleLowerCase();
        const owner = savedContactProfiles.find((profile) => profile.addresses.includes(email));
        if (owner) owners[email] = owner.id;
      }
      return owners;
    },
    async getContactProfile(id) {
      const saved = savedContactProfiles.find((profile) => profile.id === id);
      if (saved) return structuredClone(saved);
      const email = id.startsWith("derived:") ? id.slice(8) : id;
      const suggestion = contacts.find((contact) => contact.email === email);
      return suggestion ? { id: `derived:${email}`, displayName: suggestion.displayName, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: suggestion.pinned, addresses: [email], sentCount: suggestion.sentCount, receivedCount: suggestion.receivedCount, lastInteractedAt: suggestion.lastInteractedAt, birthday: null, keepInTouch: { ...NO_KEEP_IN_TOUCH }, keepInTouchDueAt: null } : null;
    },
    async saveContactProfile(request: SaveContactRequest) {
      const addresses = request.addresses.map((address) => address.trim().toLocaleLowerCase());
      const id = request.id && !request.id.startsWith("derived:") ? request.id : `contact:${addresses[0]}`;
      if (addresses.some((address) => savedContactProfiles.some((profile) => profile.id !== id && profile.addresses.includes(address)))) throw new Error("That address already belongs to another saved contact.");
      const previous = savedContactProfiles.find((profile) => profile.id === id);
      const derived = !previous && request.id?.startsWith("derived:") ? contacts.find((contact) => contact.email === addresses[0]) : undefined;
      if (request.birthday && !validDemoBirthday(request.birthday)) throw new Error("Enter a birthday as MM-DD or YYYY-MM-DD");
      // Like the backend, the form never carries keep-in-touch settings: a
      // re-save keeps them, whatever the request object holds.
      const candidate = withKeepInTouchDue({
        id, displayName: request.displayName, role: request.role, company: request.company, location: request.location, bio: request.bio, notes: request.notes,
        links: request.links, photoData: request.photoData, favorite: request.favorite, addresses, birthday: request.birthday?.trim() || null,
        sentCount: previous?.sentCount ?? derived?.sentCount ?? 0, receivedCount: previous?.receivedCount ?? derived?.receivedCount ?? 0, lastInteractedAt: previous?.lastInteractedAt ?? derived?.lastInteractedAt ?? null,
        keepInTouch: previous?.keepInTouch ?? { ...NO_KEEP_IN_TOUCH }, keepInTouchDueAt: null,
      });
      savedContactProfiles = [...savedContactProfiles.filter((profile) => profile.id !== id), candidate];
      return structuredClone(candidate);
    },
    async previewContactImport() { throw new Error("Contact file import is available in the desktop app."); },
    async exportContacts() { throw new Error("Contact file export is available in the desktop app."); },
    async importContacts(requests) {
      if (requests.length > 5000) throw new Error("Too many contacts in one import");
      const before = structuredClone(savedContactProfiles);
      let imported = 0, skipped = 0;
      try {
        for (const request of requests) {
          if (request.addresses.some(email => savedContactProfiles.some(profile => profile.addresses.includes(email.trim().toLowerCase())))) { skipped++; continue; }
          await client.saveContactProfile({ ...request, id: null }); imported++;
        }
      } catch (error) { savedContactProfiles = before; throw error; }
      return { imported, skipped };
    },
    async mergeContacts(targetId, sourceIds) {
      const sources = [...new Set(sourceIds)].sort();
      const target = structuredClone(savedContactProfiles.find(profile => profile.id === targetId));
      if (!target || !sources.length || sources.length > 50 || sources.includes(targetId)) throw new Error("Choose a retained contact and 1 to 50 other saved contacts");
      const join = (first: string | null, second: string | null) => !first ? second : !second || first === second ? first : `${first}\n\n${second}`;
      for (const id of sources) {
        const source = savedContactProfiles.find(profile => profile.id === id);
        if (!source) throw new Error("Saved contact not found");
        const conflicts: string[] = [];
        for (const [key, label] of [["displayName", "Name"], ["role", "Role"], ["company", "Company"], ["location", "Location"], ["birthday", "Birthday"]] as const) {
          if (!target[key]) target[key] = source[key];
          else if (source[key] && source[key] !== target[key]) conflicts.push(`${label}: ${source[key]}`);
        }
        target.notes = join(join(target.notes, source.notes), conflicts.length ? `Merged profile details:\n${conflicts.join("\n")}` : null);
        target.bio = join(target.bio, source.bio);
        target.addresses = [...new Set([...target.addresses, ...source.addresses])];
        target.links = [...new Set([...target.links, ...source.links])];
        target.favorite ||= source.favorite; target.photoData ??= source.photoData;
        const lastTouchAt = [target.keepInTouch.lastTouchAt, source.keepInTouch.lastTouchAt].filter((value): value is string => !!value).sort((a, b) => Date.parse(b) - Date.parse(a))[0] ?? null;
        if (!target.keepInTouch.intervalDays) target.keepInTouch = structuredClone(source.keepInTouch);
        target.keepInTouch.lastTouchAt = lastTouchAt;
      }
      if ((target.notes?.length ?? 0) > 8000 || (target.bio?.length ?? 0) > 4000 || target.links.length > 20) throw new Error("Combined profile exceeds contact limits. Shorten its notes, bio, or links before merging.");
      savedContactProfiles = [...savedContactProfiles.filter(profile => profile.id !== targetId && !sources.includes(profile.id)), withKeepInTouchDue(target)];
      contactGroups = contactGroups.map(group => ({ ...group, memberIds: [...new Set(group.memberIds.map(id => sources.includes(id) ? targetId : id))] }));
      return structuredClone(withKeepInTouchDue(target));
    },
    async listContactSuppressions() { return [...suppressedContacts].sort(); },
    async setContactSuppressed(raw, suppressed) {
      const email = raw.trim().toLowerCase();
      if (email.length > 320 || !/^[^\s@<>]+@[^\s@<>]+$/.test(email)) throw new Error("Enter one valid email address");
      if (suppressed) suppressedContacts.add(email); else suppressedContacts.delete(email);
    },
    async deleteContactProfile(id) {
      savedContactProfiles = savedContactProfiles.filter((profile) => profile.id !== id);
      contactGroups = contactGroups.map((group) => ({ ...group, memberIds: group.memberIds.filter((member) => member !== id) }));
    },
    async listContactGroups() {
      return [...contactGroups].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id.localeCompare(b.id)).map(presentGroup);
    },
    async createContactGroup(name, contactIds = [], emails = []) {
      if (contactGroups.length >= MAX_CONTACT_GROUPS) throw new Error(`You can have at most ${MAX_CONTACT_GROUPS} groups`);
      const trimmed = validGroupName(name);
      const now = new Date().toISOString();
      const group: ContactGroup = { id: `group-${crypto.randomUUID()}`, name: trimmed, memberIds: [], createdAt: now, updatedAt: now };
      contactGroups = [...contactGroups, group];
      if (!contactIds.length && !emails.length) return presentGroup(group);
      try {
        return await client.addContactGroupMembers(group.id, contactIds, emails);
      } catch (error) {
        contactGroups = contactGroups.filter((candidate) => candidate.id !== group.id);
        throw error;
      }
    },
    async listContactGroupRecipients() {
      return (await client.listContactGroups()).map((group) => ({
        id: group.id,
        name: group.name,
        members: group.memberIds.flatMap((contactId) => {
          const profile = savedContactProfiles.find((item) => item.id === contactId);
          return profile?.addresses[0] ? [{ contactId, displayName: profile.displayName, email: profile.addresses[0], addresses: [...profile.addresses] }] : [];
        }),
      }));
    },
    async renameContactGroup(id, name) {
      const group = contactGroups.find((candidate) => candidate.id === id);
      if (!group) throw new Error("Group not found");
      const next = { ...group, name: validGroupName(name, id), updatedAt: new Date().toISOString() };
      contactGroups = contactGroups.map((candidate) => candidate.id === id ? next : candidate);
      return presentGroup(next);
    },
    async addContactGroupMembers(id, contactIds, emails = []) {
      const group = contactGroups.find((candidate) => candidate.id === id);
      if (!group) throw new Error("Group not found");
      if (contactIds.length + emails.length > MAX_CONTACT_GROUP_MEMBERS) throw new Error(`A group can have at most ${MAX_CONTACT_GROUP_MEMBERS} members`);
      const resolved = await resolveGroupMembers(contactIds, emails);
      const memberIds = [...new Set([...group.memberIds, ...resolved])];
      if (memberIds.length > MAX_CONTACT_GROUP_MEMBERS) throw new Error(`A group can have at most ${MAX_CONTACT_GROUP_MEMBERS} members`);
      const next = { ...group, memberIds, updatedAt: new Date().toISOString() };
      contactGroups = contactGroups.map((candidate) => candidate.id === id ? next : candidate);
      return presentGroup(next);
    },
    async removeContactGroupMembers(id, contactIds) {
      const group = contactGroups.find((candidate) => candidate.id === id);
      if (!group) throw new Error("Group not found");
      const next = { ...group, memberIds: group.memberIds.filter((member) => !contactIds.includes(member)), updatedAt: new Date().toISOString() };
      contactGroups = contactGroups.map((candidate) => candidate.id === id ? next : candidate);
      return presentGroup(next);
    },
    async deleteContactGroup(id) { contactGroups = contactGroups.filter((group) => group.id !== id); },
    async listKeepInTouch() {
      const due = (profile: ContactProfile) => profile.keepInTouchDueAt ? Date.parse(profile.keepInTouchDueAt) : Number.POSITIVE_INFINITY;
      return structuredClone(savedContactProfiles.map(withKeepInTouchDue)
        .filter((profile) => profile.keepInTouch.intervalDays !== null || profile.birthday)
        .sort((a, b) => due(a) - due(b) || (a.displayName ?? "").localeCompare(b.displayName ?? "")));
    },
    async setKeepInTouch(ids, intervalDays) {
      if (!ids.length) throw new Error("Choose at least one contact");
      if (intervalDays !== null && (!Number.isInteger(intervalDays) || intervalDays < 1 || intervalDays > MAX_KEEP_IN_TOUCH_DAYS)) throw new Error(`Keep in touch every 1 to ${MAX_KEEP_IN_TOUCH_DAYS} days`);
      const now = new Date().toISOString();
      const updated: ContactProfile[] = [];
      for (const id of ids) {
        const current = await ensureSavedContact(id);
        const keepInTouch = intervalDays === null
          ? { ...current.keepInTouch, intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null }
          : { ...current.keepInTouch, intervalDays, startedAt: current.keepInTouch.intervalDays === null ? now : current.keepInTouch.startedAt ?? now };
        const next = withKeepInTouchDue({ ...current, keepInTouch });
        savedContactProfiles = savedContactProfiles.map((item) => item.id === next.id ? next : item);
        if (!updated.some((item) => item.id === next.id)) updated.push(next);
      }
      return structuredClone(updated);
    },
    async snoozeKeepInTouch(id, until) {
      const profile = savedContactProfiles.find((item) => item.id === id);
      if (!profile || profile.keepInTouch.intervalDays === null) throw new Error("Turn on keep in touch for this contact before snoozing");
      if (until !== null && !(Date.parse(until) > Date.now())) throw new Error("Choose a snooze date within the next two years");
      const next = withKeepInTouchDue({ ...profile, keepInTouch: { ...profile.keepInTouch, snoozedUntil: until, snoozedAt: until ? new Date().toISOString() : null } });
      savedContactProfiles = savedContactProfiles.map((item) => item.id === id ? next : item);
      return structuredClone(next);
    },
    async markContacted(id) {
      const saved = await ensureSavedContact(id);
      const next = withKeepInTouchDue({ ...saved, keepInTouch: { ...saved.keepInTouch, lastTouchAt: new Date().toISOString(), snoozedUntil: null, snoozedAt: null } });
      savedContactProfiles = savedContactProfiles.map((item) => item.id === next.id ? next : item);
      return structuredClone(next);
    },
    async listContactTasks(id): Promise<ThreadTask[]> {
      const threadIds = new Set((await this.contactTimeline(id, 0, 100)).map((item) => item.threadId));
      const tasks = await this.listTasks();
      return tasks.filter((task) => task.threadId && threadIds.has(task.threadId) && (task.status === "open" || task.status === "in_progress"));
    },
    async contactTimeline(id, offset = 0, limit = 30, accountId): Promise<ContactTimelineItem[]> {
      const profile = await client.getContactProfile(id);
      if (!profile) return [];
      const target = new Set(profile.addresses.map((address) => address.toLocaleLowerCase()));
      const items: ContactTimelineItem[] = [];
      for (const thread of threads) {
        if (accountId && thread.accountId !== accountId) continue;
        const detail = await client.getThread(thread.id);
        const matching = detail.messages.flatMap((message) => {
          const sender = parseAddress(message.sender).email.toLocaleLowerCase();
          const contactEmail = sender === thread.accountId.toLocaleLowerCase()
            ? message.recipients.flatMap(splitAddressList).map((raw) => parseAddress(raw).email.toLocaleLowerCase()).find((address) => target.has(address))
            : target.has(sender) ? sender : undefined;
          return contactEmail ? [{ contactEmail, sentAt: message.sentAt }] : [];
        }).sort((left, right) => right.sentAt.localeCompare(left.sentAt))[0];
        if (matching) items.push({ threadId: thread.id, accountId: thread.accountId, contactEmail: matching.contactEmail, subject: thread.subject, snippet: thread.snippet, sentAt: matching.sentAt, labels: thread.labels });
      }
      items.sort((a,b) => b.sentAt.localeCompare(a.sentAt));
      return structuredClone(items.slice(offset, offset + limit));
    },
    async contactActivity(id): Promise<ContactActivity> {
      const profile = await client.getContactProfile(id);
      const target = new Set((profile?.addresses ?? []).map((address) => address.toLocaleLowerCase()));
      const sent: string[] = [];
      const received: string[] = [];
      const threadIds = new Set<string>();
      for (const thread of threads) {
        for (const message of (await client.getThread(thread.id)).messages) {
          const sender = parseAddress(message.sender).email.toLocaleLowerCase();
          const toTarget = message.recipients.flatMap(splitAddressList).some((raw) => target.has(parseAddress(raw).email.toLocaleLowerCase()));
          if (target.has(sender)) received.push(message.sentAt);
          else if (sender === thread.accountId.toLocaleLowerCase() && toTarget) sent.push(message.sentAt);
          else continue;
          threadIds.add(thread.id);
        }
      }
      const all = [...sent, ...received].sort();
      return {
        sentCount: sent.length,
        receivedCount: received.length,
        threadCount: threadIds.size,
        firstAt: all[0] ?? null,
        lastSentAt: sent.sort().at(-1) ?? null,
        recentReceivedAt: received.sort().reverse().slice(0, 24),
      };
    },
    async contactFiles(id, limit) {
      const profile = await client.getContactProfile(id);
      const target = new Set((profile?.addresses ?? []).map((address) => address.toLocaleLowerCase()));
      const files: ContactFile[] = [];
      for (const thread of threads) {
        for (const message of (await client.getThread(thread.id)).messages) {
          if (!target.has(parseAddress(message.sender).email.toLocaleLowerCase())) continue;
          for (const attachment of message.attachments) {
            if (!attachment.inline && !isCalendarAttachment(attachment)) files.push({ messageId: message.id, threadId: thread.id, subject: thread.subject, sentAt: message.sentAt, attachment });
          }
        }
      }
      files.sort((left, right) => right.sentAt.localeCompare(left.sentAt));
      return structuredClone({ files: files.slice(0, limit), total: files.length });
    },
    async domainContext(domain, exclude, limit) {
      const suffix = `@${domain.toLocaleLowerCase()}`;
      const excluded = new Set(exclude.map((email) => email.toLocaleLowerCase()));
      const people = new Map<string, DomainPerson>();
      const items: ContactTimelineItem[] = [];
      for (const thread of threads) {
        let latest: ContactTimelineItem | null = null;
        for (const message of (await client.getThread(thread.id)).messages) {
          for (const raw of [message.sender, ...message.recipients.flatMap(splitAddressList)]) {
            const address = parseAddress(raw);
            const email = address.email.toLocaleLowerCase();
            if (!email.endsWith(suffix) || excluded.has(email)) continue;
            const known = people.get(email);
            if (!known || known.lastAt < message.sentAt) people.set(email, { email, displayName: address.name && address.name !== address.email ? address.name : known?.displayName ?? null, lastAt: message.sentAt });
            if (!latest || latest.sentAt < message.sentAt) latest = { threadId: thread.id, accountId: thread.accountId, contactEmail: email, subject: thread.subject, snippet: thread.snippet, sentAt: message.sentAt, labels: thread.labels };
          }
        }
        if (latest) items.push(latest);
      }
      return structuredClone({
        people: [...people.values()].sort((left, right) => right.lastAt.localeCompare(left.lastAt)).slice(0, limit),
        threads: items.sort((left, right) => right.sentAt.localeCompare(left.sentAt)).slice(0, limit),
      });
    },
    async enrichContact() { return { suggestions: [], messagesReviewed: 0, hasMore: false }; },
    async pinContact(_accountId, email, displayName) {
      const normalized = email.trim().toLocaleLowerCase();
      const existing = contacts.find((contact) => contact.email === normalized);
      if (existing) {
        existing.pinned = true;
        if (displayName) existing.displayName = displayName;
      } else {
        contacts = [
          ...contacts,
          {
            email: normalized,
            displayName,
            sentCount: 0,
            receivedCount: 0,
            lastInteractedAt: new Date().toISOString(),
            pinned: true,
          },
        ];
      }
    },
    async unpinContact(_accountId, email) {
      const normalized = email.trim().toLocaleLowerCase();
      contacts = contacts
        .map((contact) => (contact.email === normalized ? { ...contact, pinned: false } : contact))
        .filter((contact) => contact.pinned || contact.sentCount > 0 || contact.receivedCount > 0);
    },
    async unsubscribe(messageId): Promise<UnsubscribeResult> {
      const methods = findMessage(messageId)?.unsubscribe?.methods ?? [];
      if (methods.length === 0) {
        throw new Error("This message has no unsubscribe option");
      }
      return methods.includes("oneClick")
        ? { method: "oneClick", outcome: "requested", httpStatus: 200 }
        : { method: methods[0]!, outcome: "opened", httpStatus: null };
    },
    async sync() {
      status.lastSuccessfulSync = new Date().toISOString();
      return { ...status };
    },
    async flushPending() {
      return { ...status };
    },
    async syncStatus() {
      return { ...status };
    },
    async mailSyncActivity() {
      return [];
    },
    async retryFailedMutations() {
      return { ...status };
    },
    async dismissSyncProblems() {
      return { ...status };
    },
    async recoveryStatus() {
      return null;
    },
    async googleAuthStatus() {
      return { configured: false, connected: false };
    },
    async connectGoogle() {
      status.lastSuccessfulSync = new Date().toISOString();
      return { ...status };
    },
    async disconnectGoogle() {},
    async listAccounts() {
      return structuredClone(accounts);
    },
    async addAccount(provider) {
      const palette = ["#4285F4", "#34A853", "#EA4335", "#FBBC05", "#9C27B0", "#00ACC1", "#FF7043", "#5C6BC0"];
      const account: Account = {
        email: `demo-${accounts.length + 1}@example.com`,
        displayName: null,
        color: palette[accounts.length % palette.length]!,
        status: "connected",
        provider,
        sortOrder: accounts.length,
        connectedAt: new Date().toISOString(),
        lastSyncedAt: null,
      };
      accounts = [...accounts, account];
      return structuredClone(account);
    },
    async removeAccount(email) {
      accounts = accounts.filter((account) => account.email !== email);
    },
    async reconnectAccount(email) {
      const account = accounts.find((candidate) => candidate.email === email);
      if (!account) throw new Error("Account not found");
      account.status = "connected";
      return structuredClone(account);
    },
    async setAccountDisplayName(email, displayName) {
      const account = accounts.find((candidate) => candidate.email === email);
      if (!account) throw new Error("Account not found");
      const normalized = displayName?.trim() || null;
      if (normalized && (normalized.length > 200 || Array.from(normalized).some((character) => /[\u0000-\u001f\u007f]/.test(character)))) {
        throw new Error("Sender name must be 200 characters or fewer and cannot contain control characters");
      }
      account.displayName = normalized;
    },
    async setAccountColor(email, color) {
      const account = accounts.find((candidate) => candidate.email === email);
      if (!account) throw new Error("Account not found");
      account.color = color;
    },
    async reorderAccounts(emails) {
      accounts = emails
        .map((email, index) => {
          const account = accounts.find((candidate) => candidate.email === email);
          return account ? { ...account, sortOrder: index } : null;
        })
        .filter((account): account is Account => account !== null);
    },
    async listCalendarAccounts() {
      return structuredClone(calendarAccounts);
    },
    async addCalendarAccount() {
      const email = accounts[0]?.email ?? DEMO_ACCOUNT_ID;
      const account: CalendarAccount = {
        email,
        connectedAt: new Date().toISOString(),
        status: "connected",
      };
      calendarAccounts = [account];
      calendarOptions = [
        {
          id: email,
          accountId: email,
          name: "My calendar",
          primary: true,
          selected: true,
          writable: true,
        },
      ];
      return structuredClone(account);
    },
    async reconnectCalendarAccount(email) {
      const account = calendarAccounts.find((candidate) => candidate.email === email);
      if (!account) throw new Error("Calendar account not found");
      account.status = "connected";
      return structuredClone(account);
    },
    async removeCalendarAccount(email) {
      calendarAccounts = calendarAccounts.filter((account) => account.email !== email);
      calendarOptions = calendarOptions.filter((calendar) => calendar.accountId !== email);
    },
    async listCalendarOptions() {
      return structuredClone(calendarOptions);
    },
    async setCalendarSelection(accountId, calendarIds) {
      const selected = new Set(calendarIds);
      calendarOptions = calendarOptions.map((calendar) =>
        calendar.accountId === accountId
          ? { ...calendar, selected: selected.has(calendar.id) }
          : calendar
      );
      return structuredClone(calendarOptions.filter((calendar) => calendar.accountId === accountId));
    },
    async listScheduleEvents(timeMin, timeMax) {
      const connected = new Set(
        calendarAccounts.filter((account) => account.status === "connected").map((account) => account.email),
      );
      return {
        events: structuredClone(scheduleEvents
          .filter((event) => connected.has(event.accountId))
          .filter((event) => {
            const [start, end] = eventRange(event);
            return end > Date.parse(timeMin) && start < Date.parse(timeMax);
          })
          .sort((a, b) => eventRange(a)[0] - eventRange(b)[0])),
        errors: [],
      };
    },
    async updateCalendarResponse(event, responseStatus) {
      const current = scheduleEvents.find((item) => item.id === event.id && item.accountId === event.accountId);
      if (!current || !current.canRespond) throw new Error("This event has no RSVP for your calendar");
      current.responseStatus = responseStatus;
      return structuredClone(current);
    },
    async findCalendarInvitation(_uid) {
      return null;
    },
    async defaultAppStatus() {
      return { supported: false, mail: false, calendar: false };
    },
    async makeDefaultApp(_role) {
      throw new Error("Default apps can only be set from the installed ThreeStrands app on macOS");
    },
    async createCalendarEvent(request) {
      const calendar = calendarOptions.find((option) => option.id === request.calendarId && option.accountId === request.accountId && option.writable);
      if (!calendar) throw new Error("Choose a calendar where you can create events");
      if (!calendarAccounts.some((account) => account.email === request.accountId && account.status === "connected")) {
        throw new Error("Connect this calendar account in Settings first");
      }
      if (!request.title.trim() || Date.parse(request.end) <= Date.parse(request.start)) throw new Error("Enter a valid event title and time");
      const created: ScheduleEvent = {
        id: `${calendar.id}:demo-${crypto.randomUUID()}`,
        accountId: request.accountId,
        title: request.title.trim(),
        start: request.start,
        end: request.end,
        allDay: false,
        description: request.description.trim() || null,
        attendees: request.attendees.map((email) => email.toLowerCase()),
        canEdit: true,
      };
      scheduleEvents.push(created);
      return structuredClone(created);
    },
    async updateCalendarEvent(request) {
      const current = scheduleEvents.find((item) => item.id === request.eventId && item.accountId === request.accountId);
      if (!current || !current.canEdit) throw new Error("Only events you organize can be changed");
      if (!request.title.trim() || request.end <= request.start) throw new Error("Enter a valid event title and time");
      Object.assign(current, {
        title: request.title.trim(),
        start: request.start,
        end: request.end,
        allDay: request.allDay,
        location: request.location.trim() || null,
        description: request.description.trim() || null,
        attendees: request.attendees.map((email) => email.toLowerCase()),
      });
      return structuredClone(current);
    },
    async deleteCalendarEvent(event) {
      const index = scheduleEvents.findIndex((item) => item.id === event.id && item.accountId === event.accountId);
      if (index === -1 || !scheduleEvents[index].canEdit) throw new Error("Only events you organize can be changed");
      scheduleEvents.splice(index, 1);
    },
    async findAvailability(request: { rangeStart: string; rangeEnd: string; preferences: AvailabilityPreferences; maxPerDay?: number }): Promise<AvailabilityResult> {
      const start = new Date(request.rangeStart);
      const end = new Date(request.rangeEnd);
      const total = calendarOptions.filter((option) => option.selected).length;
      const candidates: AvailabilityResult["candidates"] = [];
      const perDay = new Map<string, number>();
      const busy = demoBusyIntervals();
      const now = Date.now();
      for (let cursor = new Date(start); cursor < end && candidates.length < 20; cursor.setMinutes(cursor.getMinutes() + request.preferences.slotIncrementMinutes)) {
        const weekday = cursor.getDay();
        const window = request.preferences.workingWindows.find((candidate) => candidate.weekday === weekday);
        if (!window) continue;
        const [startHour, startMinute] = window.start.split(":").map(Number);
        const [endHour, endMinute] = window.end.split(":").map(Number);
        const minutes = cursor.getHours() * 60 + cursor.getMinutes();
        if (minutes < startHour * 60 + startMinute || minutes + request.preferences.defaultDurationMinutes > endHour * 60 + endMinute || cursor.getTime() <= now) continue;
        const slotEnd = new Date(cursor.getTime() + request.preferences.defaultDurationMinutes * 60_000);
        if (busy.some(([busyStart, busyEnd]) => busyStart < slotEnd.getTime() && busyEnd > cursor.getTime())) continue;
        const day = cursor.toDateString();
        if ((perDay.get(day) ?? 0) >= (request.maxPerDay ?? 20)) continue;
        perDay.set(day, (perDay.get(day) ?? 0) + 1);
        candidates.push({
          start: cursor.toISOString(),
          end: slotEnd.toISOString(),
          status: total > 0 ? "verified" : "unverified",
        });
      }
      return { candidates, checkedCalendarCount: total, totalCalendarCount: total, errors: [] };
    },
    async checkProposedTime(request: { start: string; end: string; timeZone: string }): Promise<ProposedTimeCheck> {
      const total = calendarOptions.filter((option) => option.selected).length;
      const start = Date.parse(request.start);
      const end = Date.parse(request.end);
      const conflicts = demoBusyIntervals()
        .filter(([busyStart, busyEnd]) => busyStart < end && busyEnd > start)
        .map(([busyStart, busyEnd]) => ({ start: new Date(busyStart).toISOString(), end: new Date(busyEnd).toISOString() }));
      return {
        status: conflicts.length > 0 ? "conflicting" : total > 0 ? "free" : "unverified",
        conflicts,
        checkedCalendarCount: total,
        totalCalendarCount: total,
        errors: [],
      };
    },
    async listTasks(accountId, status) {
      return structuredClone(tasks
        .filter((task) => !accountId || accountId === "all" || task.accountId === accountId)
        .filter((task) => !status || task.status === status)
        .sort((left, right) => (left.dueValue ?? "9999").localeCompare(right.dueValue ?? "9999")));
    },
    async createTask(request: CreateTaskRequest) {
      const thread = request.threadId
        ? threads.find((candidate) => candidate.id === request.threadId)
        : null;
      if (request.threadId && !thread) throw new Error("Source thread not found");
      if (!request.title.trim()) throw new Error("Task title is required");
      ensureGoalLink(request.accountId, request.goalId);
      const now = new Date().toISOString();
      const task: ThreadTask = {
        id: `demo-task-${crypto.randomUUID()}`,
        accountId: request.accountId,
        threadId: request.threadId,
        sourceMessageId: request.sourceMessageId ?? null,
        subjectSnapshot: request.subjectSnapshot,
        title: request.title.trim(),
        notes: request.notes?.trim() || null,
        kind: request.kind,
        dueKind: request.dueKind ?? "none",
        dueValue: request.dueValue ?? null,
        timeZone: request.timeZone ?? null,
        repeatIntervalDays: request.repeatIntervalDays ?? null,
        status: "open",
        completionSource: null,
        evidenceText: request.evidenceText ?? null,
        waitAfter: thread?.lastReceivedAt ?? null,
        createdAt: now,
        updatedAt: now,
        completedAt: null,
        goalId: request.goalId ?? null,
      };
      tasks = [...tasks, task];
      return structuredClone(task);
    },
    async updateTask(request: UpdateTaskRequest) {
      const index = tasks.findIndex((task) => task.id === request.id);
      if (index === -1) throw new Error("Task not found");
      const current = tasks[index];
      if (request.goalId !== undefined) ensureGoalLink(current.accountId, request.goalId);
      const next: ThreadTask = {
        ...current,
        title: request.title?.trim() || current.title,
        notes: request.notes === undefined ? current.notes : request.notes?.trim() || null,
        kind: request.kind ?? current.kind,
        dueKind: request.dueKind ?? current.dueKind,
        dueValue: request.dueValue === undefined ? current.dueValue : request.dueValue,
        timeZone: request.timeZone === undefined ? current.timeZone : request.timeZone,
        repeatIntervalDays: request.repeatIntervalDays === undefined ? current.repeatIntervalDays : request.repeatIntervalDays,
        goalId: request.goalId === undefined ? current.goalId ?? null : request.goalId,
        updatedAt: new Date().toISOString(),
      };
      tasks = tasks.map((task, candidateIndex) => candidateIndex === index ? next : task);
      return structuredClone(next);
    },
    async setTaskStatus(id, status, source = "user") {
      const current = tasks.find((task) => task.id === id);
      if (!current) throw new Error("Task not found");
      const next = {
        ...current,
        status,
        completionSource: isActiveTaskStatus(status) ? null : source,
        completedAt: status === "completed" ? new Date().toISOString() : null,
        updatedAt: new Date().toISOString(),
      } satisfies ThreadTask;
      tasks = tasks.map((task) => task.id === id ? next : task);
      return structuredClone(next);
    },
    async recordFollowUp(id) {
      const current = tasks.find((task) => task.id === id);
      if (!current) throw new Error("Task not found");
      if (!isActiveTaskStatus(current.status) || current.kind !== "follow_up" || !current.repeatIntervalDays) {
        throw new Error("Only active repeating follow-up tasks can be recorded");
      }
      const nextDue = current.dueValue ? new Date(current.dueValue) : new Date();
      if (current.dueKind === "date") {
        const [year, month, day] = (current.dueValue ?? "").split("-").map(Number);
        nextDue.setFullYear(year, month - 1, day);
        nextDue.setHours(12, 0, 0, 0);
      }
      nextDue.setDate(nextDue.getDate() + current.repeatIntervalDays);
      const next = {
        ...current,
        dueValue: current.dueKind === "date"
          ? nextDue.toISOString().slice(0, 10)
          : nextDue.toISOString(),
        updatedAt: new Date().toISOString(),
      } satisfies ThreadTask;
      tasks = tasks.map((task) => task.id === id ? next : task);
      return structuredClone(next);
    },
    async reconcileTasks() {
      return 0;
    },
    async listGoals(accountId) {
      return structuredClone(goals.filter((goal) => !accountId || accountId === "all" || goal.accountId === accountId));
    },
    async createGoal(request: CreateGoalRequest) {
      const now = new Date().toISOString();
      const goal: Goal = {
        id: `demo-goal-${crypto.randomUUID()}`,
        accountId: request.accountId,
        title: request.title.trim(),
        notes: request.notes?.trim() || null,
        horizon: request.horizon,
        period: request.period,
        status: "active",
        parentGoalId: request.parentGoalId ?? null,
        createdAt: now,
        updatedAt: now,
        closedAt: null,
      };
      validateGoal(goal);
      goals = [...goals, goal];
      return structuredClone(goal);
    },
    async updateGoal(request: UpdateGoalRequest) {
      const current = goals.find((goal) => goal.id === request.id);
      if (!current) throw new Error("Goal not found");
      const status = request.status ?? current.status;
      const next: Goal = {
        ...current,
        title: request.title?.trim() ?? current.title,
        notes: request.notes === undefined ? current.notes : request.notes?.trim() || null,
        horizon: request.horizon ?? current.horizon,
        period: request.period ?? current.period,
        status,
        parentGoalId: request.parentGoalId === undefined ? current.parentGoalId : request.parentGoalId,
        closedAt: status === "active" ? null : status === current.status ? current.closedAt : new Date().toISOString(),
        updatedAt: new Date().toISOString(),
      };
      validateGoal(next);
      goals = goals.map((goal) => goal.id === next.id ? next : goal);
      return structuredClone(next);
    },
    async deleteGoal(id) {
      const now = new Date().toISOString();
      goals = goals.filter((goal) => goal.id !== id)
        .map((goal) => goal.parentGoalId === id ? { ...goal, parentGoalId: null, updatedAt: now } : goal);
      tasks = tasks.map((task) => task.goalId === id ? { ...task, goalId: null, updatedAt: now } : task);
    },
    async listLabels() {
      return structuredClone(labels);
    },
    async createLabel(name) {
      const normalized = name.trim();
      if (!normalized) throw new Error("Label name is required");
      const label: Label = {
        id: `demo-${crypto.randomUUID()}`,
        name: normalized,
        kind: "user",
        color: null,
      };
      labels = [...labels, label];
      return structuredClone(label);
    },
    async updateLabel(id, name) {
      const label = labels.find((candidate) => candidate.id === id);
      if (!label) throw new Error("Label not found");
      label.name = name.trim();
      return structuredClone(label);
    },
    async deleteLabel(id) {
      labels = labels.filter((label) => label.id !== id || label.kind === "system");
      threads = threads.map((thread) => ({
        ...thread,
        labels: thread.labels.filter((labelId) => labelId !== id),
      }));
    },
    async listSplitInboxes() {
      return structuredClone(splitInboxes);
    },
    async createSplitInbox(name, matchKind, matchValue, accountId) {
      const normalizedName = name.trim();
      const normalizedValue = matchValue.trim();
      if (!normalizedName) throw new Error("Split inbox name cannot be empty");
      if (!normalizedValue) throw new Error("Split inbox match value cannot be empty");
      const splitInbox: SplitInbox = {
        id: `demo-${crypto.randomUUID()}`,
        name: normalizedName,
        matchKind,
        matchValue: matchKind === "label" ? normalizedValue : normalizedValue.toLocaleLowerCase(),
        sortOrder: splitInboxes.length,
        createdAt: new Date().toISOString(),
        accountId,
      };
      splitInboxes = [...splitInboxes, splitInbox];
      return structuredClone(splitInbox);
    },
    async updateSplitInbox(id, name) {
      const splitInbox = splitInboxes.find((candidate) => candidate.id === id);
      if (!splitInbox) throw new Error("Split inbox not found");
      const normalizedName = name.trim();
      if (!normalizedName) throw new Error("Split inbox name cannot be empty");
      splitInbox.name = normalizedName;
      return structuredClone(splitInbox);
    },
    async deleteSplitInbox(id) {
      splitInboxes = splitInboxes.filter((candidate) => candidate.id !== id);
    },
    async reorderSplitInboxes(ids) {
      splitInboxes = ids
        .map((id, index) => {
          const splitInbox = splitInboxes.find((candidate) => candidate.id === id);
          return splitInbox ? { ...splitInbox, sortOrder: index } : null;
        })
        .filter((splitInbox): splitInbox is SplitInbox => splitInbox !== null);
    },
    async listSplitInboxPage(splitInboxId, offset, limit): Promise<ThreadPage> {
      const items = visibleSplitInbox(splitInboxId);
      return { threads: structuredClone(items.slice(offset, offset + limit)), hasMore: offset + limit < items.length };
    },
    async listSnippets() {
      return structuredClone(snippets);
    },
    async createSnippet(name, body) {
      const normalizedName = name.trim();
      const normalizedBody = body.trim();
      if (!normalizedName) throw new Error("Snippet name cannot be empty");
      if (!normalizedBody) throw new Error("Snippet body cannot be empty");
      const snippet: Snippet = {
        id: `demo-${crypto.randomUUID()}`,
        name: normalizedName,
        body: normalizedBody,
        createdAt: new Date().toISOString(),
      };
      snippets = [...snippets, snippet];
      return structuredClone(snippet);
    },
    async updateSnippet(id, name, body) {
      const snippet = snippets.find((candidate) => candidate.id === id);
      if (!snippet) throw new Error("Snippet not found");
      const normalizedName = name.trim();
      const normalizedBody = body.trim();
      if (!normalizedName) throw new Error("Snippet name cannot be empty");
      if (!normalizedBody) throw new Error("Snippet body cannot be empty");
      snippet.name = normalizedName;
      snippet.body = normalizedBody;
      return structuredClone(snippet);
    },
    async deleteSnippet(id) {
      snippets = snippets.filter((candidate) => candidate.id !== id);
    },
  };

  return client;
}

/**
 * `VITE_DEMO_DATASET=showcase` swaps in the marketing dataset. Vite inlines
 * the variable at build time, so a normal build drops the showcase module.
 */
export const demoClient = /* @__PURE__ */ createDemoClient(
  import.meta.env.VITE_DEMO_DATASET === "showcase" ? buildShowcaseDataset() : defaultDemoDataset(),
);

const NO_KEEP_IN_TOUCH: KeepInTouch = { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null };

const validDemoBirthday = (value: string) => {
  const match = /^(?:(\d{4})-)?(\d{2})-(\d{2})$/.exec(value.trim());
  if (!match) return false;
  const date = new Date(Number(match[1] ?? 2000), Number(match[2]) - 1, Number(match[3]));
  return date.getMonth() === Number(match[2]) - 1 && date.getDate() === Number(match[3]);
};

/** Mirrors `keep_in_touch_due_at` in `src-tauri/src/db/contacts.rs`. */
function withKeepInTouchDue(profile: ContactProfile): ContactProfile {
  const { intervalDays, startedAt, snoozedUntil, snoozedAt, lastTouchAt } = profile.keepInTouch;
  if (intervalDays === null) return { ...profile, keepInTouchDueAt: null };
  const touches = [profile.lastInteractedAt, lastTouchAt].flatMap((value) => value ? [Date.parse(value)] : []).filter(Number.isFinite);
  const lastTouch = touches.length ? Math.max(...touches) : null;
  if (snoozedUntil && !(lastTouch !== null && snoozedAt && lastTouch > Date.parse(snoozedAt))) return { ...profile, keepInTouchDueAt: snoozedUntil };
  const base = lastTouch ?? (startedAt ? Date.parse(startedAt) : null);
  return { ...profile, keepInTouchDueAt: base === null ? null : new Date(base + intervalDays * 86_400_000).toISOString() };
}
