export type Thread = {
  id: string;
  providerThreadId: string;
  subject: string;
  snippet: string;
  participants: string[];
  lastMessageAt: string;
  unread: boolean;
  starred: boolean;
  archived: boolean;
  trashed: boolean;
  labels: string[];
  accountId: string;
  /** FTS5 match excerpt with hits wrapped in MATCH_START/MATCH_END markers. Only set on search results. */
  matchSnippet?: string | null;
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
