import { demoCorrespondence } from "./demoCorrespondence";
import type { MailClient } from "./client";
import type { Label, SyncStatus, Thread, ThreadDetail, ThreadMutation } from "../domain";

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
    labels: ["INBOX"],
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
    labels: ["INBOX", "STARRED"],
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
    labels: ["INBOX"],
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

function visible(): Thread[] {
  return threads
    .filter((thread) => !thread.archived)
    .sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt));
}

function update(mutation: ThreadMutation) {
  threads = threads.map((thread) => {
    if (thread.id !== mutation.threadId) return thread;
    switch (mutation.kind) {
      case "archive":
        return { ...thread, archived: mutation.value };
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
  ...demoCorrespondence((id) => demoClient.getThread(id)),
  async listThreads() {
    return structuredClone(visible());
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
        },
      ],
    };
    return detail;
  },
  async searchThreads({ query, limit = 50 }) {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return this.listThreads();
    return structuredClone(
      visible()
        .filter((thread) =>
          [thread.subject, thread.snippet, ...thread.participants]
            .join(" ")
            .toLocaleLowerCase()
            .includes(normalized),
        )
        .slice(0, limit),
    );
  },
  async mutateThread(mutation) {
    update(mutation);
  },
  async sync() {
    status.lastSuccessfulSync = new Date().toISOString();
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
