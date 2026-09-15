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

export type SyncStatus = {
  state: "idle" | "syncing" | "offline" | "error";
  lastSuccessfulSync: string | null;
  cursor: string | null;
  pendingMutations: number;
  error: string | null;
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

export type CrashReport = {
  id: string;
  occurredAt: string;
  kind: "error" | "unhandledrejection";
  message: string;
  stack: string | null;
  appVersion: string;
  userAgent: string;
};
