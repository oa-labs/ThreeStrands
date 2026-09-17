import { demoCorrespondence } from "./demoCorrespondence";
import type { MailClient } from "./client";
import { parseAddress } from "../emailAddress";
import type {
  Account,
  ContactSuggestion,
  Label,
  ReplyAssistContext,
  ReplyAssistResult,
  SplitInbox,
  SummaryResult,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadPage,
  ThreadMutation,
  TriageEvent,
  TriageSenderStats,
  UnsubscribeResult,
} from "../domain";

export const DEMO_ACCOUNT_ID = "demo@example.com";

const initialThreads: Thread[] = [
  {
    id: "welcome",
    providerThreadId: "demo-welcome",
    subject: "Welcome to Dispatch",
    snippet: "A keyboard-first inbox that keeps your mail on this device.",
    participants: ["Dispatch"],
    lastMessageAt: "2026-03-05T16:30:00Z",
    lastReceivedAt: "2026-03-05T16:30:00Z",
    unread: true,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
    hasAttachments: true,
  },
  {
    id: "roadmap",
    providerThreadId: "demo-roadmap",
    subject: "Phase 1: read and triage",
    snippet: "The first vertical slice includes local search and optimistic actions.",
    participants: ["Product Team"],
    lastMessageAt: "2026-03-05T14:15:00Z",
    lastReceivedAt: "2026-03-05T14:15:00Z",
    unread: false,
    starred: true,
    archived: false,
    trashed: false,
    labels: ["INBOX", "STARRED"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
    hasAttachments: false,
  },
  {
    id: "privacy",
    providerThreadId: "demo-privacy",
    subject: "Your inbox stays local",
    snippet: "Dispatch connects directly to Gmail and stores its cache in SQLite.",
    participants: ["Security"],
    lastMessageAt: "2026-03-04T19:40:00Z",
    lastReceivedAt: "2026-03-04T19:40:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
    hasAttachments: false,
  },
];

let accounts: Account[] = [
  {
    email: DEMO_ACCOUNT_ID,
    displayName: null,
    color: "#4285F4",
    status: "connected",
    sortOrder: 0,
    connectedAt: "2026-03-04T00:00:00Z",
    lastSyncedAt: null,
  },
];

let threads = structuredClone(initialThreads);
let labels: Label[] = [
  { id: "INBOX", name: "Inbox", kind: "system", color: null },
  { id: "STARRED", name: "Starred", kind: "system", color: null },
  { id: "work", name: "Work", kind: "user", color: "#7b73ee" },
];
let splitInboxes: SplitInbox[] = [];

const details: Record<string, string> = {
  welcome: `
    <p>Welcome to <strong>Dispatch</strong>.</p>
    <p>Use <kbd>j</kbd> and <kbd>k</kbd> to move, <kbd>e</kbd> to archive,
    <kbd>s</kbd> to star, and <kbd>⌘K</kbd> to open the command palette.</p>
    <img src="https://example.invalid/tracker.gif" alt="Blocked remote image" />
  `,
  roadmap: `
    <p>The read-and-triage milestone is built around a single rule:</p>
    <blockquote>Every interaction is local first; Gmail reconciliation follows.</blockquote>
    <p>This browser preview uses demo data. The Tauri build uses SQLite through
    the same typed client boundary.</p>
  `,
  privacy: `
    <p>No Dispatch backend is required. OAuth credentials belong in your OS
    keychain and message data belongs in the local SQLite database.</p>
  `,
};

// Stands in for locally-mined send/receive history in the browser preview —
// no imported address book, same as the real client.
let contacts: ContactSuggestion[] = [
  { email: "jane@example.com", displayName: "Jane Doe", sentCount: 12, receivedCount: 5, lastInteractedAt: "2026-03-05T12:00:00Z", pinned: false },
  { email: "product@example.com", displayName: "Product Team", sentCount: 4, receivedCount: 9, lastInteractedAt: "2026-03-05T14:15:00Z", pinned: false },
  { email: "alex@example.com", displayName: "Alex Rivera", sentCount: 1, receivedCount: 1, lastInteractedAt: "2026-02-20T09:00:00Z", pinned: false },
];

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

export const demoClient: MailClient = {
  ...demoCorrespondence((id) => demoClient.getThread(id), () => accounts[0]?.email ?? DEMO_ACCOUNT_ID),
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
      messages: [
        {
          id: `${id}-message`,
          threadId: id,
          sender: `${thread.participants[0]} <hello@dispatch.local>`,
          recipients: ["You <you@example.com>"],
          sentAt: thread.lastMessageAt,
          bodyHtml: details[id] ?? `<p>${thread.snippet}</p>`,
          bodyText: thread.snippet,
          unread: thread.unread,
          unsubscribe: id === "welcome" ? { methods: ["oneClick"], listId: "dispatch.example" } : null,
          attachments: id === "welcome"
            ? [{ id: "demo-guide", filename: "dispatch-shortcuts.txt", mimeType: "text/plain", size: 94 }]
            : [],
        },
      ],
    };
    return detail;
  },
  async openAttachment(_messageId, _attachmentId) {
    const url = URL.createObjectURL(new Blob(["Dispatch keyboard shortcuts\n\nj/k: move\ne: archive\ns: star\n"], { type: "text/plain" }));
    window.open(url, "_blank", "noopener,noreferrer");
    window.setTimeout(() => URL.revokeObjectURL(url), 60_000);
  },
  async saveAttachment(_messageId, _attachmentId) {
    const url = URL.createObjectURL(new Blob(["Dispatch keyboard shortcuts\n\nj/k: move\ne: archive\ns: star\n"], { type: "text/plain" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "dispatch-shortcuts.txt";
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
  async replyAssistContext(draftId): Promise<ReplyAssistContext> {
    const replyDraft = (await this.listDrafts()).find((candidate) => candidate.id === draftId);
    if (!replyDraft || !["reply", "replyAll"].includes(replyDraft.mode) || !replyDraft.sourceId) {
      throw new Error("Reply Assist is only available for reply drafts");
    }
    const detail = await this.getThread(replyDraft.sourceId.replace(/-message$/, ""));
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
    if (!messageId.endsWith("-message") || !messageId.startsWith("welcome")) {
      throw new Error("This message has no unsubscribe option");
    }
    return { method: "oneClick", outcome: "requested", httpStatus: 200 };
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
};
