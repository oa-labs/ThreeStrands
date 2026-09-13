import { demoCorrespondence } from "./demoCorrespondence";
import type { MailClient } from "./client";
import type {
  Account,
  Label,
  SummaryResult,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
  UnsubscribeResult,
} from "../domain";

const DEMO_ACCOUNT_ID = "demo@example.com";

const initialThreads: Thread[] = [
  {
    id: "welcome",
    providerThreadId: "demo-welcome",
    subject: "Welcome to Dispatch",
    snippet: "A keyboard-first inbox that keeps your mail on this device.",
    participants: ["Dispatch"],
    lastMessageAt: "2026-03-05T16:30:00Z",
    unread: true,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
  },
  {
    id: "roadmap",
    providerThreadId: "demo-roadmap",
    subject: "Phase 1: read and triage",
    snippet: "The first vertical slice includes local search and optimistic actions.",
    participants: ["Product Team"],
    lastMessageAt: "2026-03-05T14:15:00Z",
    unread: false,
    starred: true,
    archived: false,
    trashed: false,
    labels: ["INBOX", "STARRED"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
  },
  {
    id: "privacy",
    providerThreadId: "demo-privacy",
    subject: "Your inbox stays local",
    snippet: "Dispatch connects directly to Gmail and stores its cache in SQLite.",
    participants: ["Security"],
    lastMessageAt: "2026-03-04T19:40:00Z",
    unread: false,
    starred: false,
    archived: false,
    trashed: false,
    labels: ["INBOX"],
    accountId: DEMO_ACCOUNT_ID,
    summary: null,
    summaryGeneratedAt: null,
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

const status: SyncStatus = {
  state: "idle",
  lastSuccessfulSync: null,
  cursor: "demo",
  pendingMutations: 0,
  error: null,
};

function visibleWhere(accountId: string | undefined, predicate: (thread: Thread) => boolean): Thread[] {
  const connected = new Set(accounts.map((account) => account.email));
  return threads
    .filter((thread) => connected.has(thread.accountId))
    .filter(predicate)
    .filter((thread) => !accountId || accountId === "all" || thread.accountId === accountId)
    .sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt));
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
    return structuredClone(visible(accountId));
  },
  async listAllMail(accountId) {
    return structuredClone(visibleAllMail(accountId));
  },
  async listTrash(accountId) {
    return structuredClone(visibleTrash(accountId));
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
        },
      ],
    };
    return detail;
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
  async searchThreads({ query, limit = 50, offset = 0, includeArchived = false }, accountId) {
    if (!query.trim()) return this.listThreads(accountId);
    const pool = includeArchived
      ? [...threads]
          .filter((thread) => !accountId || accountId === "all" || thread.accountId === accountId)
          .sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt))
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
};
