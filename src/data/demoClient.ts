import { demoCorrespondence } from "./demoCorrespondence";
import { isActiveTaskStatus } from "../taskViews";
import type { MailClient } from "./client";
import { DEMO_ACCOUNT_ID, defaultDemoDataset, type DemoDataset } from "./demoDataset";
import { buildShowcaseDataset } from "./showcaseDataset";
import { parseAddress, splitAddressList } from "../emailAddress";
import type {
  Account,
  ActionAnalysis,
  AiUsageDay,
  AvailabilityPreferences,
  AvailabilityResult,
  CalendarAccount,
  ContactProfile,
  ContactTimelineItem,
  SaveContactRequest,
  CreateTaskRequest,
  Label,
  Message,
  ReplyAssistContext,
  ReplyAssistResult,
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
  let snippets = seed.snippets;
  let contacts = seed.contacts;
  let savedContactProfiles = seed.contactProfiles;
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
      const summary = [
        `- ${thread.subject}`,
        `- Latest message from ${thread.participants[0] ?? "a participant"}`,
        `- ${thread.snippet}`,
      ].join("\n");
      const generatedAt = new Date().toISOString();
      threads = threads.map((candidate) =>
        candidate.id === threadId ? { ...candidate, summary, summaryGeneratedAt: generatedAt } : candidate,
      );
      return { summary, generatedAt };
    },
    async analyzeThread(threadId): Promise<ActionAnalysis> {
      const detail = await this.getThread(threadId);
      const latest = detail.messages.at(-1);
      if (!latest) return { proposals: [], hiddenCount: 0 };
      await new Promise((resolve) => setTimeout(resolve, 400));
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
    },
    async threadChat(request): Promise<ThreadChatReply> {
      const detail = await this.getThread(request.threadId);
      await new Promise((resolve) => setTimeout(resolve, 400));
      const wantsReply = /\b(draft|write|reply)\b/i.test(request.question);
      const wantsTimes = /\b(free|available|availability|when can|find a time)\b/i.test(request.question);
      const now = new Date();
      return {
        availability: wantsTimes
          ? { rangeStart: now.toISOString(), rangeEnd: new Date(now.getTime() + 7 * 86_400_000).toISOString(), durationMinutes: 30 }
          : null,
        answer: `In the demo, answers come from “${detail.thread.subject}”: ${detail.thread.snippet}`,
        analysis: { proposals: [], hiddenCount: 0 },
        replyDraft: wantsReply ? "Thanks for the update. I'll take a look and get back to you soon." : null,
        sources: [],
        searched: [],
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
    async briefThread(threadId, userTimeZone, provider, model, endpoint): Promise<ThreadBriefResult> {
      const [summary, analysis] = await Promise.all([
        this.summarizeThread(threadId, provider, model, endpoint),
        this.analyzeThread(threadId, userTimeZone, provider, model, endpoint),
      ]);
      return { summary, analysis };
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
      const matches = contacts.filter(
        (contact) => {
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
        byEmail.set(email, { id: `derived:${email}`, displayName: item.displayName, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: item.pinned, addresses: [email], sentCount: item.sentCount, receivedCount: item.receivedCount, lastInteractedAt: item.lastInteractedAt });
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
      return suggestion ? { id: `derived:${email}`, displayName: suggestion.displayName, role: null, company: null, location: null, bio: null, notes: null, links: [], photoData: null, favorite: suggestion.pinned, addresses: [email], sentCount: suggestion.sentCount, receivedCount: suggestion.receivedCount, lastInteractedAt: suggestion.lastInteractedAt } : null;
    },
    async saveContactProfile(request: SaveContactRequest) {
      const addresses = request.addresses.map((address) => address.trim().toLocaleLowerCase());
      const id = request.id && !request.id.startsWith("derived:") ? request.id : `contact:${addresses[0]}`;
      if (addresses.some((address) => savedContactProfiles.some((profile) => profile.id !== id && profile.addresses.includes(address)))) throw new Error("That address already belongs to another saved contact.");
      const previous = savedContactProfiles.find((profile) => profile.id === id);
      const candidate = { ...request, id, addresses, sentCount: previous?.sentCount ?? 0, receivedCount: previous?.receivedCount ?? 0, lastInteractedAt: previous?.lastInteractedAt ?? null };
      savedContactProfiles = [...savedContactProfiles.filter((profile) => profile.id !== id), candidate];
      return structuredClone(candidate);
    },
    async deleteContactProfile(id) { savedContactProfiles = savedContactProfiles.filter((profile) => profile.id !== id); },
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
    async addAccount() {
      const palette = ["#4285F4", "#34A853", "#EA4335", "#FBBC05", "#9C27B0", "#00ACC1", "#FF7043", "#5C6BC0"];
      const account: Account = {
        email: `demo-${accounts.length + 1}@example.com`,
        displayName: null,
        color: palette[accounts.length % palette.length]!,
        status: "connected",
        provider: "gmail",
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
      };
      scheduleEvents.push(created);
      return structuredClone(created);
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
      };
      tasks = [...tasks, task];
      return structuredClone(task);
    },
    async updateTask(request: UpdateTaskRequest) {
      const index = tasks.findIndex((task) => task.id === request.id);
      if (index === -1) throw new Error("Task not found");
      const current = tasks[index];
      const next: ThreadTask = {
        ...current,
        title: request.title?.trim() || current.title,
        notes: request.notes === undefined ? current.notes : request.notes?.trim() || null,
        kind: request.kind ?? current.kind,
        dueKind: request.dueKind ?? current.dueKind,
        dueValue: request.dueValue === undefined ? current.dueValue : request.dueValue,
        timeZone: request.timeZone === undefined ? current.timeZone : request.timeZone,
        repeatIntervalDays: request.repeatIntervalDays === undefined ? current.repeatIntervalDays : request.repeatIntervalDays,
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
export const demoClient = createDemoClient(
  import.meta.env.VITE_DEMO_DATASET === "showcase" ? buildShowcaseDataset() : defaultDemoDataset(),
);
