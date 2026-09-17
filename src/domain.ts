export type Thread = {
  id: string;
  providerThreadId: string;
  subject: string;
  snippet: string;
  participants: string[];
  lastMessageAt: string;
  /** Newest *inbound* message's timestamp; sending a reply doesn't change this. Drives inbox sort order. */
  lastReceivedAt: string;
  unread: boolean;
  starred: boolean;
  archived: boolean;
  trashed: boolean;
  labels: string[];
  accountId: string;
  /** FTS5 match excerpt with hits wrapped in MATCH_START/MATCH_END markers. Only set on search results. */
  matchSnippet?: string | null;
  summary: string | null;
  summaryGeneratedAt: string | null;
  hasAttachments: boolean;
};

export type MessageAttachment = {
  id: string;
  filename: string;
  mimeType: string;
  size: number;
  contentId?: string | null;
  inline?: boolean;
};

export type CalendarEventPreview = {
  uid: string | null;
  title: string;
  start: string | null;
  end: string | null;
  allDay: boolean;
  timeZone: string | null;
  location: string | null;
  description: string | null;
  organizer: string | null;
  attendeeCount: number;
  recurring: boolean;
  status: string | null;
};

export type CalendarPreview = {
  events: CalendarEventPreview[];
  truncated: boolean;
};

export type Message = {
  id: string;
  threadId: string;
  sender: string;
  recipients: string[];
  sentAt: string;
  bodyHtml: string;
  bodyText: string;
  unread: boolean;
  unsubscribe?: UnsubscribeInfo | null;
  attachments: MessageAttachment[];
};

export type UnsubscribeMethod = "oneClick" | "mailto" | "web";

export type UnsubscribeInfo = {
  methods: UnsubscribeMethod[];
  listId: string | null;
};

export type UnsubscribeResult = {
  method: UnsubscribeMethod;
  outcome: "requested" | "opened";
  httpStatus: number | null;
};

export type ThreadDetail = {
  thread: Thread;
  messages: Message[];
};

export type TriageEvent = {
  threadId: string;
  kind: "open" | "close" | "disposition" | "restore" | "response";
  context: "inbox" | "other";
  action?: "archive" | "trash";
  opened?: boolean;
  dwellMs?: number | null;
  scrolled?: boolean;
  batch?: boolean;
};

export type TriageSenderStats = {
  accountId: string;
  senderEmail: string;
  senderDomain: string;
  exposureCount: number;
  engagedViewCount: number;
  dispositionCount: number;
  archiveCount: number;
  trashCount: number;
  quickDispositionCount: number;
  batchDispositionCount: number;
  restoreCount: number;
  responseCount: number;
  quickDispositionRate: number;
  lastSeenAt: string;
};

/** A past correspondent ranked for compose autocomplete, mined from local send/receive history plus anything pinned. */
export type ContactSuggestion = {
  email: string;
  displayName: string | null;
  sentCount: number;
  receivedCount: number;
  lastInteractedAt: string;
  pinned: boolean;
};

export type ThreadPage = {
  threads: Thread[];
  hasMore: boolean;
};

export type SummaryResult = {
  summary: string;
  generatedAt: string;
};

export type ReplyAssistMessage = {
  sender: string;
  sentAt: string;
  bodyText: string;
};

/** The exact, bounded mailbox content that will be sent to the selected AI provider. */
export type ReplyAssistContext = {
  subject: string;
  messages: ReplyAssistMessage[];
};

export type ReplyAssistResult = {
  body: string;
};

export type SearchThreadsRequest = {
  query: string;
  limit?: number;
  offset?: number;
  includeArchived?: boolean;
};

export type ThreadMutation =
  | { kind: "archive"; threadId: string; value: boolean }
  | { kind: "trash"; threadId: string; value: boolean }
  | { kind: "spam"; threadId: string; value: boolean }
  | { kind: "read"; threadId: string; value: boolean }
  | { kind: "star"; threadId: string; value: boolean }
  | { kind: "label"; threadId: string; labelId: string; value: boolean };

export type Label = {
  id: string;
  name: string;
  kind: "system" | "user" | string;
  color?: string | null;
};

export type SplitInboxMatchKind = "domain" | "label" | "pattern";

/** Scoped to one account — a split inbox never pulls mail out of a different account's inbox, even when viewing every account merged together. */
export type SplitInbox = {
  id: string;
  name: string;
  matchKind: SplitInboxMatchKind;
  matchValue: string;
  sortOrder: number;
  createdAt: string;
  accountId: string;
};

export type SyncStatus = {
  state: "idle" | "syncing" | "offline" | "error";
  lastSuccessfulSync: string | null;
  cursor: string | null;
  pendingMutations: number;
  failedMutations: FailedMutation[];
  quarantinedMessages: QuarantinedMessage[];
  error: string | null;
};

export type QuarantinedMessage = {
  messageId: string;
  threadId: string;
  error: string;
  createdAt: string;
};

/// Reported once at startup only when the local cache had to be recovered
/// (restored from a backup, or recreated fresh) — see `db::open_with_recovery`
/// on the Rust side. `null` (from `recoveryStatus()`) means the database
/// opened normally and no recovery happened.
export type RecoveryStatus =
  | { kind: "restoredFromBackup"; corruptPath: string | null; backupPath: string }
  | { kind: "freshDatabase"; corruptPath: string | null };

export type FailedMutation = {
  id: string;
  kind: ThreadMutation["kind"];
  threadId: string;
  attempts: number;
  error: string;
  createdAt: string;
};

export type AuthStatus = {
  configured: boolean;
  connected: boolean;
};

export type Account = {
  email: string;
  displayName: string | null;
  color: string;
  status: "connected" | "needs_reauth";
  sortOrder: number;
  connectedAt: string;
  lastSyncedAt: string | null;
};

/** Inbox unread thread totals keyed by account email. Accounts with no unread mail may be omitted. */
export type UnreadCounts = Record<string, number>;

/**
 * Unread totals for the mailbox tab bar, scoped to one account (or merged
 * across all accounts). `inbox` excludes threads claimed by any split
 * inbox rule; `splits` is keyed by split inbox id.
 */
export type MailboxUnreadCounts = {
  inbox: number;
  splits: Record<string, number>;
};

export type CrashReport = {
  id: string;
  occurredAt: string;
  kind: "error" | "unhandledrejection";
  message: string;
  stack: string | null;
  appVersion: string;
  userAgent: string;
};
