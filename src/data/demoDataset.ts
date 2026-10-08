import type {
  ContactGroup,
  Goal,
  Account,
  ActionAnalysis,
  ThreadChatReply,
  CalendarAccount,
  CalendarOption,
  ContactProfile,
  ContactSuggestion,
  Label,
  MessageAttachment,
  ScheduleEvent,
  Snippet,
  SplitInbox,
  Thread,
  ThreadTask,
  UnsubscribeInfo,
} from "../domain";

export const DEMO_ACCOUNT_ID = "demo@example.com";

/** One message in a seeded demo thread; ids are derived from the thread id and position. */
export type DemoMessageSeed = {
  sender: string;
  recipients: string[];
  sentAt: string;
  bodyHtml: string;
  bodyText: string;
  unsubscribe?: UnsubscribeInfo | null;
  attachments?: MessageAttachment[];
};

/**
 * Everything the browser-preview client starts with. Threads without an
 * entry in `messages` get a single message synthesized from `details` (or
 * their snippet), which is all the default test dataset needs.
 */
export type DemoDataset = {
  accounts: Account[];
  threads: Thread[];
  messages: Record<string, DemoMessageSeed[]>;
  details: Record<string, string>;
  labels: Label[];
  contacts: ContactSuggestion[];
  /** Birthday and keep-in-touch fields default to unset; the due date is always derived. */
  contactProfiles: (Omit<ContactProfile, "birthday" | "keepInTouch" | "keepInTouchDueAt"> & Partial<Pick<ContactProfile, "birthday" | "keepInTouch">>)[];
  splitInboxes: SplitInbox[];
  tasks: ThreadTask[];
  goals?: Goal[];
  contactGroups?: ContactGroup[];
  snippets: Snippet[];
  calendarAccounts: CalendarAccount[];
  calendarOptions: CalendarOption[];
  scheduleEvents: ScheduleEvent[];
  /** Optional fictional AI outputs for marketing; absent from the default test dataset. */
  aiFixtures?: Record<string, { summary?: string; analysis?: ActionAnalysis; chat?: Omit<ThreadChatReply, "attachments"> }>;
};

/**
 * The dataset the unit and e2e suites are written against. Its threads,
 * copy, and counts are part of the test contract — marketing screenshots
 * use `buildShowcaseDataset` instead of growing this one.
 */
export function defaultDemoDataset(): DemoDataset {
  return {
    accounts: [
      {
        email: DEMO_ACCOUNT_ID,
        displayName: null,
        color: "#4285F4",
        status: "connected",
        provider: "gmail",
        sortOrder: 0,
        connectedAt: "2026-03-04T00:00:00Z",
        lastSyncedAt: null,
      },
    ],
    threads: [
      {
        id: "welcome",
        providerThreadId: "demo-welcome",
        subject: "Welcome to ThreeStrands",
        snippet: "A keyboard-first inbox that keeps your mail on this device.",
        participants: ["ThreeStrands"],
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
        summaryRevision: null,
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
        summaryRevision: null,
        hasAttachments: false,
      },
      {
        id: "privacy",
        providerThreadId: "demo-privacy",
        subject: "Your inbox stays local",
        snippet: "ThreeStrands connects directly to Gmail and stores its cache in SQLite.",
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
        summaryRevision: null,
        hasAttachments: false,
      },
    ],
    messages: {
      welcome: [
        {
          sender: "ThreeStrands <hello@threestrands.local>",
          recipients: ["You <you@example.com>"],
          sentAt: "2026-03-05T16:30:00Z",
          bodyHtml: `
    <p>Welcome to <strong>ThreeStrands</strong>.</p>
    <p>Use <kbd>j</kbd> and <kbd>k</kbd> to move, <kbd>e</kbd> to archive,
    <kbd>s</kbd> to star, and <kbd>⌘K</kbd> to open the command palette.</p>
    <img src="https://example.invalid/tracker.gif" alt="Blocked remote image" />
  `,
          bodyText: "A keyboard-first inbox that keeps your mail on this device.",
          unsubscribe: { methods: ["oneClick"], listId: "threestrands.example" },
          attachments: [{ id: "demo-guide", filename: "threestrands-shortcuts.txt", mimeType: "text/plain", size: 94 }],
        },
      ],
    },
    details: {
      roadmap: `
    <p>The read-and-triage milestone is built around a single rule:</p>
    <blockquote>Every interaction is local first; Gmail reconciliation follows.</blockquote>
    <p>This browser preview uses demo data. The Tauri build uses SQLite through
    the same typed client boundary.</p>
  `,
      privacy: `
    <p>No ThreeStrands backend is required. OAuth credentials belong in your OS
    keychain and message data belongs in the local SQLite database.</p>
  `,
    },
    labels: [
      { id: "INBOX", name: "Inbox", kind: "system", color: null },
      { id: "STARRED", name: "Starred", kind: "system", color: null },
      { id: "work", name: "Work", kind: "user", color: "#7b73ee" },
    ],
    // Stands in for locally-mined send/receive history in the browser preview —
    // no imported address book, same as the real client.
    contacts: [
      { email: "jane@example.com", displayName: "Jane Doe", sentCount: 12, receivedCount: 5, lastInteractedAt: "2026-03-05T12:00:00Z", pinned: false },
      { email: "product@example.com", displayName: "Product Team", sentCount: 4, receivedCount: 9, lastInteractedAt: "2026-03-05T14:15:00Z", pinned: false },
      { email: "alex@example.com", displayName: "Alex Rivera", sentCount: 1, receivedCount: 1, lastInteractedAt: "2026-02-20T09:00:00Z", pinned: false },
    ],
    contactProfiles: [],
    splitInboxes: [],
    tasks: [],
    snippets: [],
    calendarAccounts: [],
    calendarOptions: [],
    scheduleEvents: [],
  };
}
