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
  /** The `lastMessageAt` the summary was written from; null for summaries saved before it was recorded. */
  summaryRevision: string | null;
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
  /**
   * A time zone the native parser could not resolve. `start` and `end` are
   * then wall-clock times in it rather than exact moments. Null when the
   * times carry a UTC offset (or are floating or all-day).
   */
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

/** A `.ics` file macOS opened with ThreeStrands, parsed natively. */
export type OpenedCalendarFile = {
  name: string;
  preview: CalendarPreview | null;
  /** Why the file could not be shown, when `preview` is null. */
  error: string | null;
};

export type DefaultAppRole = "mail" | "calendar";

/** Whether ThreeStrands is the macOS default for each role. */
export type DefaultAppStatus = {
  /** False outside an installed macOS copy, where defaults cannot be set. */
  supported: boolean;
  mail: boolean;
  calendar: boolean;
};

export type CalendarAccount = {
  email: string;
  connectedAt: string;
  status: "connected" | "needs_reauth";
};

export type CalendarOption = {
  id: string;
  accountId: string;
  name: string;
  primary: boolean;
  selected: boolean;
  writable: boolean;
};

/** All-day events carry `YYYY-MM-DD` dates with an exclusive end; timed events carry ISO times. */
export type CreateCalendarEventRequest = {
  accountId: string;
  calendarId: string;
  title: string;
  start: string;
  end: string;
  /** Absent for a timed event. */
  allDay?: boolean;
  location?: string;
  description: string;
  attendees: string[];
  /** The email the event is added from, kept privately on the event to recognize it later. */
  sourceMessageId?: string;
};

/** All-day events carry `YYYY-MM-DD` dates with an exclusive end; timed events carry ISO times. */
export type UpdateCalendarEventRequest = {
  accountId: string;
  calendarId: string;
  eventId: string;
  title: string;
  start: string;
  end: string;
  allDay: boolean;
  location: string;
  description: string;
  attendees: string[];
};

export type ScheduleEvent = {
  id: string;
  accountId: string;
  calendarId?: string;
  responseStatus?: "accepted" | "declined" | "tentative" | "needsAction" | null;
  canRespond?: boolean;
  /** The user organizes this event on a calendar they can write to. */
  canEdit?: boolean;
  title: string;
  start: string;
  end: string;
  allDay: boolean;
  location?: string | null;
  description?: string | null;
  conferenceUrl?: string | null;
  /** Lowercased addresses of the other people on the event, when known. */
  attendees?: string[];
  /** The email ThreeStrands added this event from, when it did. */
  sourceMessageId?: string | null;
};

export type ScheduleResult = {
  events: ScheduleEvent[];
  errors: string[];
};

export type AvailabilityWindow = {
  weekday: number;
  start: string;
  end: string;
};

export type AvailabilityPreferences = {
  timeZone: string;
  workingWindows: AvailabilityWindow[];
  defaultDurationMinutes: number;
  slotIncrementMinutes: number;
  driveTimeBufferMinutes: number;
};

export type AvailabilityCandidate = {
  start: string;
  end: string;
  status: "verified" | "partiallyChecked" | "unverified";
};

export type AvailabilityResult = {
  candidates: AvailabilityCandidate[];
  checkedCalendarCount: number;
  totalCalendarCount: number;
  errors: string[];
};

export type ProposedTimeCheck = {
  status: "free" | "conflicting" | "partiallyChecked" | "unverified";
  conflicts: { start: string; end: string }[];
  checkedCalendarCount: number;
  totalCalendarCount: number;
  errors: string[];
};

export type TaskKind = "action" | "follow_up" | "waiting_for";
export type TaskStatus = "open" | "in_progress" | "completed" | "cancelled";
export type TaskDueKind = "none" | "date" | "datetime";

export type ThreadTask = {
  id: string;
  accountId: string;
  threadId: string | null;
  sourceMessageId?: string | null;
  subjectSnapshot: string | null;
  title: string;
  notes?: string | null;
  kind: TaskKind;
  dueKind: TaskDueKind;
  dueValue?: string | null;
  timeZone?: string | null;
  repeatIntervalDays?: number | null;
  status: TaskStatus;
  completionSource?: "user" | "reply" | "external" | null;
  evidenceText?: string | null;
  waitAfter?: string | null;
  createdAt: string;
  updatedAt: string;
  completedAt?: string | null;
  /** The goal this task supports, in the same account. */
  goalId?: string | null;
};

export type CreateTaskRequest = {
  accountId: string;
  threadId: string | null;
  sourceMessageId?: string | null;
  subjectSnapshot: string | null;
  title: string;
  notes?: string | null;
  kind: TaskKind;
  dueKind?: TaskDueKind;
  dueValue?: string | null;
  timeZone?: string | null;
  repeatIntervalDays?: number | null;
  evidenceText?: string | null;
  goalId?: string | null;
};

export type UpdateTaskRequest = {
  id: string;
  title?: string;
  notes?: string | null;
  kind?: TaskKind;
  dueKind?: TaskDueKind;
  dueValue?: string | null;
  timeZone?: string | null;
  repeatIntervalDays?: number | null;
  goalId?: string | null;
};

export type GoalHorizon = "year" | "half" | "quarter";
export type GoalStatus = "active" | "achieved" | "dropped";

/** A long-term goal for one account. `period` is one period of its horizon: `2026`, `2026-H2`, or `2026-Q4`. */
export type Goal = {
  id: string;
  accountId: string;
  title: string;
  notes: string | null;
  horizon: GoalHorizon;
  period: string;
  status: GoalStatus;
  /** A goal of a longer horizon, in an enclosing period, that this one supports. */
  parentGoalId: string | null;
  createdAt: string;
  updatedAt: string;
  closedAt: string | null;
};

export type CreateGoalRequest = {
  accountId: string;
  title: string;
  notes?: string | null;
  horizon: GoalHorizon;
  period: string;
  parentGoalId?: string | null;
};

export type UpdateGoalRequest = {
  id: string;
  title?: string;
  notes?: string | null;
  horizon?: GoalHorizon;
  period?: string;
  status?: GoalStatus;
  parentGoalId?: string | null;
};

export type Message = {
  id: string;
  threadId: string;
  sender: string;
  /** The To addresses. */
  recipients: string[];
  /** The Cc addresses; Bcc is never shown. Absent from older stored data. */
  cc?: string[];
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

export type ContactProfile = {
  id: string;
  displayName: string | null;
  role: string | null;
  company: string | null;
  location: string | null;
  bio: string | null;
  notes: string | null;
  links: string[];
  photoData: string | null;
  favorite: boolean;
  addresses: string[];
  sentCount: number;
  receivedCount: number;
  lastInteractedAt: string | null;
  /** `MM-DD`, or `YYYY-MM-DD` when the year is known. */
  birthday: string | null;
  keepInTouch: KeepInTouch;
  /** When the next keep-in-touch reminder falls due; derived on every read. */
  keepInTouchDueAt: string | null;
};

/** A group member as a compose recipient: `email` is the primary address; `addresses` lists every one. */
export type GroupRecipient = { contactId: string; displayName: string | null; email: string; addresses: string[] };
/** A group with its members resolved for compose. */
export type ContactGroupRecipients = { id: string; name: string; members: GroupRecipient[] };

/** A named set of saved contacts, shared by every account. */
export type ContactGroup = {
  id: string;
  name: string;
  /** Members whose contact is on this device, by name. */
  memberIds: string[];
  createdAt: string;
  updatedAt: string;
};

/** Keep-in-touch reminder settings stored on a saved contact. */
export type KeepInTouch = {
  /** Days between touches; null turns reminders off. */
  intervalDays: number | null;
  startedAt: string | null;
  snoozedUntil: string | null;
  snoozedAt: string | null;
  /** Latest touch logged by hand rather than by email. */
  lastTouchAt: string | null;
};

export type ContactTimelineItem = {
  threadId: string;
  accountId: string;
  /** The matching contact address on this conversation's latest interaction. */
  contactEmail: string;
  subject: string;
  snippet: string;
  sentAt: string;
  labels: string[];
};

/** Local correspondence history with one person; automated mail is excluded. */
export type ContactActivity = {
  sentCount: number;
  receivedCount: number;
  threadCount: number;
  firstAt: string | null;
  lastSentAt: string | null;
  /** Newest first, one entry per received message. */
  recentReceivedAt: string[];
};

/**
 * A muted sender in one account: sync marks their new mail read and archives
 * it. `address` matches one address exactly; `domain` matches anyone at it.
 */
export type SenderRule = {
  id: string;
  accountId: string;
  matchKind: "address" | "domain";
  matchValue: string;
  action: "mute";
  createdAt: string;
};

/** A new mute, and the Inbox conversations it archived, for Undo. */
export type MuteResult = {
  rule: SenderRule;
  archivedThreadIds: string[];
  /** Those of them that were unread before. */
  unreadThreadIds: string[];
};

/** Everything stored from one sender address in one account, including the automated mail contact history leaves out. */
export type SenderHistory = {
  receivedCount: number;
  /** Received messages no longer marked unread. */
  readCount: number;
  firstAt: string | null;
  /** The user's latest message to the address, from this account. */
  lastSentAt: string | null;
  /** Newest first, one entry per received message. */
  recentReceivedAt: string[];
  /** Conversations from the sender still in the Inbox, newest first. */
  inboxThreadIds: string[];
  /** Conversations from the sender outside Trash, newest first. */
  threads: SenderThread[];
};

/** One conversation in a sender's history, dated by the sender's newest message in it. */
export type SenderThread = { threadId: string; subject: string; snippet: string; sentAt: string };

/**
 * Verdicts the user's receiving server recorded in the topmost
 * Authentication-Results header: "pass" when any result for a method passed,
 * otherwise the first one reported ("fail", "softfail", "none", ...).
 */
export type AuthenticationVerdicts = {
  spf: string | null;
  dkim: string | null;
  dmarc: string | null;
  /** Signing domains of the DKIM signatures that passed. */
  dkimDomains: string[];
  /** Envelope sender (smtp.mailfrom) domains of the SPF checks that passed. */
  spfDomains: string[];
};

/** Trust signals read from a message's stored headers. */
export type MessageTrust = {
  /** The From address's domain; empty when From has no address. */
  fromDomain: string;
  /** Null when the receiving server reported no authentication results. */
  authentication: AuthenticationVerdicts | null;
  /** The Reply-To address, only when it belongs to another organization than From. */
  replyTo: string | null;
};

/** How well the user knows a message's sender, read from the local cache and contacts. */
export type SenderFamiliarity = {
  /** Messages the user sent to the address, from any account. */
  sentCount: number;
  firstSentAt: string | null;
  /** Whether mail from the address arrived before this message. */
  receivedBefore: boolean;
  /** The saved contact's name, when the address belongs to a saved contact. */
  contactName: string | null;
  /** Other addresses the user knows under the same display name. */
  nameMatches: string[];
  /** Domains of saved contacts' addresses and of addresses the user has written to. */
  knownDomains: string[];
};

/** One attachment a person sent, with the message it arrived on. */
export type ContactFile = {
  messageId: string;
  threadId: string;
  subject: string;
  sentAt: string;
  attachment: MessageAttachment;
};

export type ContactFiles = { files: ContactFile[]; total: number };

/** One attachment occurrence, never identified by filename or size. */
export type WorkspaceFile = ContactFile & {
  accountId: string;
  sender: string;
  direction: "received" | "sent";
  starred: boolean;
  availability: "offline" | "download" | "unknown";
  reason?: string;
};
export type FileQuery = {
  accountIds: string[];
  threadId?: string;
  threadIds?: string[];
  priorityMessageId?: string;
  prioritizeStarred?: boolean;
  search?: string;
  sender?: string;
  after?: string;
  before?: string;
  type?: string;
  direction?: "received" | "sent";
  starred?: boolean;
  /** Leave calendar invitations out; the page reports how many in `hiddenCalendar`. */
  excludeCalendar?: boolean;
  offset?: number;
  limit?: number;
};
export type FilePage = { files: WorkspaceFile[]; total: number; hiddenCalendar?: number };
export type WorkspaceContextTarget = { kind: "task"; task: ThreadTask } | { kind: "meeting"; event: ScheduleEvent };
export type ContextQuery = { accountIds: string[]; threadId?: string; terms: string[]; attendees: string[]; after?: string; before?: string };
export type WorkspaceConversation = { threadId: string; accountId: string; subject: string; sentAt: string; participants: string[]; reason?: string };


/** Keep-in-touch settings change only through their own commands, never through the profile form. */
export type SaveContactRequest = Omit<ContactProfile, "sentCount" | "receivedCount" | "lastInteractedAt" | "id" | "keepInTouch" | "keepInTouchDueAt"> & { id: string | null };
export type ContactFieldSuggestion = { field: "displayName" | "role" | "company" | "location" | "bio" | "link"; value: string; sourceMessageId: string; sourceThreadId: string; excerpt: string };
export type ContactEnrichmentResult = { suggestions: ContactFieldSuggestion[]; messagesReviewed: number; hasMore: boolean };

export type ThreadPage = {
  threads: Thread[];
  hasMore: boolean;
};

export type SummaryResult = {
  summary: string;
  generatedAt: string;
  /** The thread's `lastMessageAt` the summary was written from. */
  revision: string;
};

/** One local day's provider usage for one provider and model. */
export type AiUsageDay = {
  day: string;
  provider: string;
  model: string;
  requests: number;
  inputTokens: number;
  outputTokens: number;
  /** Requests whose provider reported a price; their total is `reportedCostUsd`. */
  reportedCostRequests: number;
  reportedCostUsd: number;
};

/** One earlier exchange in a thread chat. */
export type ChatTurn = { role: "user" | "assistant"; content: string };

/** A question about the open conversation; mailbox search applies to it alone. */
export type ThreadChatRequest = {
  threadId: string;
  question: string;
  history: ChatTurn[];
  searchMailbox: boolean;
  includeProposals: boolean;
  contactId: string | null;
  userTimeZone: string;
  /** Attachments in this conversation the user chose to share for the question. */
  attachments: ChatAttachmentRef[];
};

/** An attachment in the open conversation, by message and attachment id. */
export type ChatAttachmentRef = { messageId: string; attachmentId: string };

/** An attachment whose text was shared with the provider; `truncated` means only its start was. */
export type ChatAttachmentSource = ChatAttachmentRef & { filename: string; truncated: boolean };

/** A range the chat asked the app to search; times always come from the calendar. */
/** A search Thread Chat asked the app to run, limited like Find Times to some weekdays and hours. */
export type ChatAvailability = {
  rangeStart: string; rangeEnd: string; durationMinutes: number | null;
  weekdays: number[]; dayStart: string | null; dayEnd: string | null;
};

/** Another conversation shared with the provider while answering. */
export type ChatSource = { threadId: string; accountId: string; subject: string; lastMessageAt: string };

export type ThreadChatReply = {
  answer: string;
  analysis: ActionAnalysis;
  replyDraft: string | null;
  /** Conversations the answer says it relied on. */
  sources: ChatSource[];
  /** Every other conversation shared because the question searched all mail. */
  searched: ChatSource[];
  /** Every attachment whose text was shared for this question. */
  attachments: ChatAttachmentSource[];
  availability: ChatAvailability | null;
};

/** One combined request: the persisted summary and the verified suggestions. */
export type ThreadBriefResult = {
  summary: SummaryResult;
  analysis: ActionAnalysis;
};

export type ProposalEvidence = {
  sourceMessageId: string;
  excerpt: string;
};

export type MeetingProposal = {
  type: "meeting";
  intent: string;
  title: string;
  participants: string[];
  location: string | null;
  rawTimeLanguage: string;
  normalizedStart: string | null;
  normalizedEnd: string | null;
  /** A `YYYY-MM-DD` day for an event with a date but no time; absent from earlier suggestions. */
  eventDate?: string | null;
  searchRangeStart: string | null;
  searchRangeEnd: string | null;
  durationMinutes: number | null;
  timeZone: string | null;
  confidence: number;
  evidence: ProposalEvidence;
};

export type TaskProposal = {
  type: "task";
  kind: TaskKind;
  title: string;
  notes: string | null;
  dueKind: TaskDueKind;
  dueValue: string | null;
  timeZone: string | null;
  repeatIntervalDays: number | null;
  /** A goal the suggested task would support; only a goal of the thread's account that the app listed. */
  goalId?: string | null;
  confidence: number;
  evidence: ProposalEvidence;
};

export type ActionProposal = MeetingProposal | TaskProposal;

/**
 * Verified proposals plus how many the provider returned that failed
 * validation, and how many valid ones fell past the per-analysis limit.
 */
export type ActionAnalysis = {
  proposals: ActionProposal[];
  hiddenCount: number;
  extraCount?: number;
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

/**
 * The days and hours Find Times searches, read from a conversation by AI.
 * The times themselves always come from the user's calendar.
 */
export type TimeSearch = {
  rangeStart: string;
  rangeEnd: string;
  durationMinutes: number | null;
  /** Allowed weekdays, 0 for Sunday; empty allows every day. */
  weekdays: number[];
  /** Earliest local start, `HH:MM`. */
  dayStart: string | null;
  /** Latest local end, `HH:MM`. */
  dayEnd: string | null;
  /** One line telling the user what the search is based on. */
  reason: string;
};

export type FindTimesRequest = {
  /** The message the draft replies to or forwards, whose conversation is read. */
  sourceMessageId: string | null;
  draftSubject: string;
  draftText: string;
  /** The user's change to `current`; both are set or neither. */
  instruction: string | null;
  current: TimeSearch | null;
  userTimeZone: string;
};

export type DraftReviewRequest = {
  subject: string;
  body: string;
  goal: string;
};

export type DraftReviewResult = {
  assessment: string;
  suggestions: {
    title: string;
    field: "subject" | "body";
    excerpt: string;
    reason: string;
    replacement: string;
  }[];
  revisedSubject: string;
  revisedBody: string;
};

export type SearchThreadsRequest = {
  query: string;
  limit?: number;
  offset?: number;
  includeArchived?: boolean;
};

// Phase 5 run 5: what a server search was able to do, carried out of the
// backend as typed CODES (never developer strings, mailbox wire names, or the
// user's query). The UI maps these codes to copy in `searchStatusCopy.ts`, so
// the exact wording is testable in vitest. Mirrors the Rust `SearchReport`
// serde shape (camelCase; `status` is internally tagged on `kind`, each reason
// tagged on `code`).
export type PartialSearchReason =
  | { code: "deadline" }
  | { code: "budget" }
  | { code: "folderError" };

export type SkipSearchReason = { code: "unsupported" };

export type NoMatchSearchReason = { code: "unknownName" };

export type SearchReportStatus =
  | { kind: "complete" }
  | { kind: "partial"; reasons: PartialSearchReason[] }
  | { kind: "skipped"; reasons: SkipSearchReason[] }
  | { kind: "noServerMatches"; reason: NoMatchSearchReason };

export type SearchReport = {
  status: SearchReportStatus;
  /** Planned folders skipped because they are not synced (summed across accounts). */
  unsyncedMailboxes: number;
  /** Hits refused because keeping them would exceed the search-pin cap (summed). */
  skippedOverCap: number;
  /** Accounts that actually ran a server search (summed). Zero means none did. */
  serverSearchedAccounts: number;
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

/** Global (not per-account) — unlike a `SplitInbox`, a snippet has no owning account. */
export type Snippet = {
  id: string;
  name: string;
  body: string;
  createdAt: string;
};

export type SyncStatus = {
  state: "idle" | "syncing" | "offline" | "error";
  lastSuccessfulSync: string | null;
  cursor: string | null;
  pendingMutations: number;
  failedMutations: FailedMutation[];
  quarantinedMessages: QuarantinedMessage[];
  error: string | null;
  /** Accounts whose latest check for new mail failed. Failed queued actions are not included. */
  fetchErrors?: AccountSyncError[];
};

export type AccountSyncError = {
  accountId: string;
  error: string;
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

/** A mail backend an account can authenticate and sync through. */
export type MailProvider = "gmail" | "imap";

/**
 * How an account models the labels a user can see and act on. Drives which
 * label actions the UI offers, so it never presents one the provider can only
 * reject (see `labelCapabilities`). Mirrors the Rust `LabelModel`.
 * - `gmailLabels`: Gmail's many-to-many labels — create, rename, delete, toggle.
 * - `imapKeywords`: IMAP keyword user labels (not yet writable in the app).
 * - `imapLabelFolders`: IMAP label-folder user labels — create, delete, toggle,
 *   but NOT rename (the folder id is derived from the name, so a rename can
 *   only fail and roll back).
 * - `imapNoUserLabels`: an IMAP account with no user-label storage — no label
 *   actions at all.
 */
export type LabelModel = "gmailLabels" | "imapKeywords" | "imapLabelFolders" | "imapNoUserLabels";

/** How a mail connection reaches TLS. Mirrors the Rust `SecurityMode`. */
export type ImapSecurityMode = "implicit_tls" | "start_tls";

/** How an IMAP account stores user labels. */
export type ImapLabelStorage = "keywords" | "folders" | "none";

/** One discovered server endpoint. */
export type DiscoveredServer = {
  host: string;
  port: number;
  security: ImapSecurityMode;
  username: string;
};

/** Autodiscovery result: proposed IMAP + SMTP settings for confirmation. */
export type ImapDiscoveryResult = {
  imap: DiscoveredServer;
  smtp: DiscoveredServer;
  source: string;
};

/** What the cert-trust step shows about an untrusted certificate. */
export type ImapCertificateInfo = {
  subject: string;
  issuer: string;
  sha256Fingerprint: string;
};

/** The certificate probe outcome for the cert-trust step. */
export type ImapCertificateProbe = {
  certificate: ImapCertificateInfo;
  /** True when the cert already chains to a public root (no pin needed). */
  trustedByPlatform: boolean;
};

/** One of the app's system mailbox roles discovery maps a server mailbox onto. Mirrors the Rust `MailboxRole`. */
export type ImapMailboxRole = "sent" | "archive" | "drafts" | "trash" | "junk" | "all";

/** How a role's mailbox was chosen: an authoritative attribute, or a name guess to confirm. */
export type ImapMappingSource = "special_use" | "name_match";

/** One role's proposed mailbox and how it was chosen. */
export type ImapRoleProposal = {
  role: ImapMailboxRole;
  mailbox: string;
  source: ImapMappingSource;
};

/** One selectable mailbox, for the mapping screen's override pickers. */
export type ImapDiscoveredMailbox = {
  name: string;
  delimiter: string | null;
  specialUse: string | null;
};

/** The proposed role mapping plus the selectable-mailbox list the mapping screen renders. */
export type ImapMailboxMapping = {
  proposals: ImapRoleProposal[];
  selectable: ImapDiscoveredMailbox[];
  /** Potential label containers, including non-selectable hierarchy nodes. */
  labelContainers: ImapDiscoveredMailbox[];
};

/** The confirmed/edited mapping the user commits on the mapping screen. */
export type ImapMailboxMappingCommit = {
  email: string;
  /** The chosen Archive mailbox name, if an existing one was selected. */
  archive: string | null;
  /** Create a new Archive mailbox with this name instead of choosing one. */
  createArchive: string | null;
  /** Confirmed role → mailbox-name choices, keyed by role (`sent`, `trash`, …). */
  mailboxOverrides: Record<string, string>;
  /** Updated label-folder container mailbox, if changed. */
  labelContainer: string | null;
};

/** One address an account may send as, with an optional display name. The first in an account's list is its primary (login) address. */
export type SendingIdentity = { address: string; displayName?: string };

/** An account's sending options for the Settings "Sending" panel (IMAP only). */
export type ImapSendingOptions = {
  /** Ordered send-as identities (login/sign-in address first), with display names. */
  identities: SendingIdentity[];
  /** The user override: true = the server saves its own sent mail, so don't save a copy. */
  serverSavesSentOverride: boolean;
  /** This device's local Sent-copy detection: true server saves its own, false it does not, null not yet decided. */
  sentCopyDetected: boolean | null;
};

/** An IMAP account's non-secret server settings for the Settings panel. */
export type ImapConnectionSettings = {
  imapHost: string;
  imapPort: number;
  imapSecurity: ImapSecurityMode;
  imapUsername: string;
  smtpHost: string;
  smtpPort: number;
  smtpSecurity: ImapSecurityMode;
  smtpUsername: string;
  /** Whether a self-signed certificate is pinned for the current endpoint. */
  imapCertificatePinned: boolean;
  smtpCertificatePinned: boolean;
};

/** Edited server settings for an existing IMAP account. */
export type ImapConnectionUpdate = {
  email: string;
  imapHost: string;
  imapPort: number;
  imapSecurity: ImapSecurityMode;
  imapUsername: string;
  smtpHost: string;
  smtpPort: number;
  smtpSecurity: ImapSecurityMode;
  smtpUsername: string;
  /** A new password; null keeps the stored one. */
  password: string | null;
  /** A newly trusted certificate fingerprint; null keeps any existing pin for the same host and port. */
  imapPinnedFingerprint: string | null;
  smtpPinnedFingerprint: string | null;
};

/** One submitted sending-identity row from the Settings edit form. */
export type SendingIdentityInput = { address: string; displayName?: string | null };

/** The payload the IMAP setup form sends for "test and save". */
export type ImapSetupRequest = {
  email: string;
  imapHost: string;
  imapPort: number;
  imapSecurity: ImapSecurityMode;
  imapUsername: string;
  imapPassword: string;
  smtpHost: string;
  smtpPort: number;
  smtpSecurity: ImapSecurityMode;
  smtpUsername: string;
  smtpPassword: string | null;
  imapPinnedFingerprint: string | null;
  smtpPinnedFingerprint: string | null;
  labelStorage: ImapLabelStorage;
  labelContainer: string | null;
};

export type Account = {
  email: string;
  displayName: string | null;
  /** Private label for this account; independent of the outgoing sender name. */
  nickname?: string | null;
  color: string;
  status: "connected" | "needs_reauth";
  /** Which backend this account authenticates and syncs through. */
  provider: MailProvider;
  /** How this account models user-visible labels. The UI reads this (via
   * `labelCapabilities`) to hide label actions the provider can only reject,
   * rather than branching on provider names or label ids in components. */
  labelModel: LabelModel;
  sortOrder: number;
  connectedAt: string;
  lastSyncedAt: string | null;
  /** Whether the account is set up on THIS device. False for an account
   * another device synchronized in (or one disconnected here): it is listed
   * as available to set up, and stays out of the switcher and mail views. */
  onThisDevice: boolean;
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

export type ContactFormat = "csv" | "vcard";
export interface ContactImportPreview {
  contacts: SaveContactRequest[];
  warnings: string[];
  skipped: number;
}
export interface ContactImportResult { imported: number; skipped: number }
