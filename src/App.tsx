import {
  Archive,
  AlertCircle,
  CalendarDays,
  Check,
  CheckCircle2,
  CheckSquare,
  ChevronDown,
  ChevronUp,
  Command as CommandIcon,
  Copy,
  FileText,
  Send,
  Reply,
  ReplyAll,
  Forward,
  Inbox,
  Keyboard,
  ListFilter,
  Mail,
  Mails,
  MailOpen,
  Moon,
  Download,
  ExternalLink,
  Paperclip,
  Plus,
  Sun,
  Pencil,
  RefreshCw,
  RotateCcw,
  Search,
  Settings as SettingsIcon,
  ShieldAlert,
  Sparkles,
  Square,
  Star,
  Tag,
  Trash2,
  Unlink,
  Upload,
  X,
} from "lucide-react";
import {
  type CSSProperties,
  type RefObject,
  useCallback,
  useEffect,
  memo,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  accountCommand,
  commands,
  isEditableTarget,
  labelCommand,
  matchesShortcut,
  shortcutSteps,
  showAllAccountsCommand,
  splitInboxCommand,
  undoResult,
  type Command,
  type CommandContext,
  type CommandResult,
  type MailboxKind,
} from "./commands";
import {
  clearLocalCrashReports,
  crashReportingEnabled,
  localCrashReports,
  setCrashReportingEnabled,
} from "./crashReporting";
import { mailClient } from "./data/client";
import { createForegroundRefreshController } from "./foregroundRefresh";
import { formatLabelName, labelIdsForConversationDisplay } from "./labels";
import {
  filterThreadsByMessageFilters,
  MESSAGE_FILTER_OPTIONS,
  type MessageFilterKind,
} from "./messageFilters";
import { formattingShortcuts } from "./richText";
import type {
  Account,
  AuthStatus,
  CalendarAccount,
  Label,
  RecoveryStatus,
  SplitInbox,
  SplitInboxMatchKind,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
  TriageEvent,
  UnreadCounts,
  Message,
} from "./domain";
import { InboxResizeHandle, useInboxWidth } from "./InboxResizeHandle";
import { DraftsList, OutboxList, useCorrespondence } from "./useCorrespondence";
import type { Draft, OutboxItem } from "./correspondence";
import { decodeHtmlEntities, SafeMessage } from "./SafeMessage";
import { CalendarAttachmentGroup, isCalendarAttachment } from "./CalendarAttachment";
import { CalendarSidebar } from "./CalendarSidebar";
import { isInlineImageAttachment, normalizeContentId, referencedImageContentIds } from "./inlineAttachments";
import { formatDisplayName, parseAddress, splitAddressList } from "./emailAddress";
import {
  FONT_SCALE_STEP,
  MAX_FONT_SCALE,
  MIN_FONT_SCALE,
} from "./fontScale";

import type { Theme } from "./theme";
import {
  DEFAULT_FONT_FAMILY,
  fontFamilyStack,
  MAX_AUTO_READ_DELAY_SECONDS,
  MIN_AUTO_READ_DELAY_SECONDS,
  readSelectedTabForAccount,
  saveSelectedTabForAccount,
  type FontFamily,
} from "./settings";
import { listSystemFontFamilies } from "./systemFonts";
import {
  AI_MODEL_PLACEHOLDERS,
  AI_PROVIDER_OPTIONS,
  clearAiApiKey,
  isAiApiKeyConfigured,
  readAiEndpoint,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  resolveAiModel,
  saveAiEndpoint,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  setAiApiKey,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { getRetentionDays, setRetentionDays, RETENTION_OPTIONS } from "./retentionSettings";
import { exportSettings, importSettings, type SettingsImportResult } from "./userPreferences";
import { useAccounts } from "./useAccounts";
import { useAppPreferences } from "./useAppPreferences";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { useReaderState } from "./useReaderState";
import {
  buildTriageCloseEvent,
  buildTriageDispositionEvent,
  pauseTriageSession,
  resumeTriageSession,
  type TriageSession,
} from "./triage";

type SettingsSection = "appearance" | "reading" | "accounts" | "calendarAccounts" | "splitInboxes" | "ai" | "privacy" | "data";

type Notice = { message: string; undo?: () => void };

export const NOTICE_TIMEOUT_MS = 6000;

let noticeSequence = 0;

function useNotice() {
  const [notice, setCurrent] = useState<(Notice & { key: number }) | null>(null);
  const timeout = useRef<number | null>(null);

  const clearTimer = useCallback(() => {
    if (timeout.current === null) return;
    window.clearTimeout(timeout.current);
    timeout.current = null;
  }, []);

  const setNotice = useCallback((next: Notice | null) => {
    clearTimer();
    if (!next) {
      setCurrent(null);
      return;
    }
    const key = ++noticeSequence;
    setCurrent({ ...next, key });
    timeout.current = window.setTimeout(() => {
      timeout.current = null;
      setCurrent((shown) => (shown?.key === key ? null : shown));
    }, NOTICE_TIMEOUT_MS);
  }, [clearTimer]);

  useEffect(() => clearTimer, [clearTimer]);

  return [notice, setNotice] as const;
}

const timeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
});

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  month: "short",
  day: "2-digit",
});

export function formatMailTimestamp(iso: string, now = new Date()): string {
  const date = new Date(iso);
  const isToday =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  return isToday ? timeFormatter.format(date) : dateFormatter.format(date);
}

const SEARCH_PAGE_SIZE = 50;

const MAILBOX_TITLES: Record<MailboxKind, string> = {
  inbox: "Inbox",
  allMail: "All Mail",
  trash: "Trash",
  drafts: "Drafts",
  outbox: "Outbox",
  split: "Split Inbox",
};

// Matches the \u{1}/\u{2} markers the backend's FTS5 `snippet()` call wraps
// hits in (see search_threads in src-tauri/src/db.rs). Rendered as React
// elements rather than HTML so a match can never inject markup.
const MATCH_START = "";
const MATCH_END = "";

function HighlightedSnippet({ thread }: { thread: Thread }) {
  const raw = thread.matchSnippet;
  if (!raw) return <>{decodeHtmlEntities(thread.snippet)}</>;
  const segments = decodeHtmlEntities(raw).split(MATCH_START);
  return (
    <>
      {segments[0]}
      {segments.slice(1).map((segment, index) => {
        const [match, ...rest] = segment.split(MATCH_END);
        return (
          <span key={index}>
            <mark>{match}</mark>
            {rest.join(MATCH_END)}
          </span>
        );
      })}
    </>
  );
}

function formatAttachmentSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.ceil(size / 1024)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

const ThreadRow = memo(function ThreadRow({
  thread,
  selected,
  checked,
  accountColor,
  showAccount,
  onSelect,
  onToggleCheck,
  rowRef,
}: {
  thread: Thread;
  selected: boolean;
  checked: boolean;
  accountColor?: string;
  showAccount: boolean;
  onSelect(id: string): void;
  onToggleCheck(id: string): void;
  rowRef?: RefObject<HTMLButtonElement | null>;
}) {
  return (
    <button
      ref={rowRef}
      role="option"
      aria-selected={selected}
      className={`thread-row ${selected ? "selected" : ""}`}
      onClick={() => onSelect(thread.id)}
    >
      <span className="row-leading">
        <span
          className={`row-check ${checked ? "checked" : ""}`}
          aria-hidden="true"
          onClick={(event) => {
            event.stopPropagation();
            onToggleCheck(thread.id);
          }}
        >
          {checked ? <CheckSquare size={16} /> : <Square size={16} />}
        </span>
        {thread.hasAttachments ? (
          <Paperclip className="thread-attachment" size={13} aria-label="Has attachments" />
        ) : null}
      </span>
      {checked ? <span className="sr-only">Selected for batch actions</span> : null}
      <span className={`unread-dot ${thread.unread ? "visible" : ""}`} />
      <span className="thread-content">
        <span className="thread-meta">
          <span className="thread-sender">
            <strong>
              {thread.participants
                .map((participant) => formatDisplayName(parseAddress(participant).name))
                .join(", ")}
            </strong>
          </span>
          <span className="thread-meta-trailing">
            {showAccount ? <span className="account-dot" aria-hidden="true" style={{ background: accountColor }} /> : null}
            <time>{formatMailTimestamp(thread.lastMessageAt)}</time>
          </span>
        </span>
        <span className="thread-subject">{thread.subject}</span>
        <span className="thread-snippet"><HighlightedSnippet thread={thread} /></span>
      </span>
      {thread.starred ? <Star className="starred" size={15} fill="currentColor" /> : null}
    </button>
  );
});

type MutationTemplate =
  | { kind: "archive"; value: boolean }
  | { kind: "trash"; value: boolean }
  | { kind: "spam"; value: boolean }
  | { kind: "read"; value: boolean }
  | { kind: "star"; value: boolean }
  | { kind: "label"; labelId: string; value: boolean };

function buildThreadMutation(threadId: string, template: MutationTemplate): ThreadMutation {
  return template.kind === "label"
    ? { kind: "label", threadId, labelId: template.labelId, value: template.value }
    : { kind: template.kind, threadId, value: template.value };
}

function applyMutationTemplate(thread: Thread, template: MutationTemplate): Thread {
  switch (template.kind) {
    case "archive":
      return { ...thread, archived: template.value };
    case "trash":
      return { ...thread, trashed: template.value };
    case "spam": {
      const next = new Set(thread.labels);
      if (template.value) {
        next.add("SPAM");
        next.delete("INBOX");
      } else {
        next.delete("SPAM");
        next.add("INBOX");
      }
      return { ...thread, archived: template.value, labels: [...next] };
    }
    case "read":
      return { ...thread, unread: !template.value };
    case "star":
      return { ...thread, starred: template.value };
    case "label": {
      const next = new Set(thread.labels);
      if (template.value) next.add(template.labelId);
      else next.delete(template.labelId);
      return { ...thread, labels: [...next] };
    }
  }
}

function invertMutationTemplate(template: MutationTemplate): MutationTemplate {
  return { ...template, value: !template.value } as MutationTemplate;
}

function describeMutation(template: MutationTemplate, count: number, labelName?: string): string {
  const many = count > 1;
  switch (template.kind) {
    case "archive":
      return template.value
        ? (many ? `Archived ${count} conversations` : "Conversation archived")
        : (many ? `Marked ${count} conversations as not done` : "Conversation marked as not done");
    case "trash":
      return template.value
        ? (many ? `Moved ${count} conversations to trash` : "Conversation moved to trash")
        : (many ? `Restored ${count} conversations from trash` : "Conversation restored from trash");
    case "spam":
      return template.value
        ? (many ? `Marked ${count} conversations as spam` : "Conversation marked as spam")
        : (many ? `Restored ${count} conversations from spam` : "Conversation restored from spam");
    case "star":
      return template.value
        ? (many ? `Starred ${count} conversations` : "Starred")
        : (many ? `Unstarred ${count} conversations` : "Unstarred");
    case "read":
      return template.value
        ? (many ? `Marked ${count} conversations as read` : "Marked as read")
        : (many ? `Marked ${count} conversations as unread` : "Marked as unread");
    case "label": {
      const name = labelName ?? "Label";
      return template.value
        ? (many ? `${name} added to ${count} conversations` : `${name} added`)
        : (many ? `${name} removed from ${count} conversations` : `${name} removed`);
    }
  }
}

function sortByRecency(threads: Thread[]): Thread[] {
  return [...threads].sort((a, b) => b.lastReceivedAt.localeCompare(a.lastReceivedAt));
}

function triageNow(): number {
  return typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();
}

function useShortcutHandler(
  context: CommandContext,
  execute: (command: Command) => void,
  extraCommands: Command[] = [],
) {
  const contextRef = useRef(context);
  contextRef.current = context;
  const extraRef = useRef(extraCommands);
  extraRef.current = extraCommands;
  const pendingStep = useRef<string | null>(null);
  const pendingTimeout = useRef<number | null>(null);

  useEffect(() => {
    const clearPendingStep = () => {
      pendingStep.current = null;
      if (pendingTimeout.current !== null) {
        window.clearTimeout(pendingTimeout.current);
        pendingTimeout.current = null;
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      const currentContext = contextRef.current;
      if (currentContext.closing || event.isComposing || event.defaultPrevented) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        clearPendingStep();
        event.preventDefault();
        currentContext.openPalette();
        return;
      }
      const sendShortcut = event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && (event.metaKey || event.ctrlKey) && event.key === "Enter";
      const replyAssistShortcut = event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "j";
      const fontShortcut = (event.metaKey || event.ctrlKey) && ["=", "+", "-"].includes(event.key);
      const dialog = document.querySelector('[role="dialog"]');
      const allowsMailboxNavigation = dialog?.classList.contains("correspondence-list");
      // Tab/Shift+Tab double as the split-inbox tab cycler, but only when
      // focus isn't already on some other focusable control — otherwise
      // this would hijack Tab away from normal focus-cycling between
      // buttons/links/checkboxes, breaking keyboard/screen-reader navigation.
      const focusedControl = event.key === "Tab"
        && event.target instanceof HTMLElement
        && event.target !== document.body
        && event.target.matches("button, a[href], [tabindex]");
      if (!sendShortcut && !replyAssistShortcut && !fontShortcut && (isEditableTarget(event.target) || focusedControl || (dialog && !allowsMailboxNavigation))) {
        clearPendingStep();
        return;
      }

      const allCommands = [...commands, ...extraRef.current];

      if (pendingStep.current) {
        const command = allCommands.find(
          (candidate) =>
            candidate.enabled(currentContext) &&
            candidate.keys.some((key) => {
              const steps = shortcutSteps(key);
              return steps.length === 2 &&
                steps[0].toLocaleLowerCase() === pendingStep.current &&
                matchesShortcut(event, steps[1]);
            }),
        );
        clearPendingStep();
        if (command) {
          event.preventDefault();
          execute(command);
          return;
        }
      }

      const command = allCommands.find(
        (candidate) =>
          candidate.enabled(currentContext) &&
          candidate.keys.some((key) => {
            const steps = shortcutSteps(key);
            return steps.length === 1 && matchesShortcut(event, steps[0]);
          }),
      );
      if (command) {
        event.preventDefault();
        execute(command);
        return;
      }

      const prefix = allCommands
        .filter((candidate) => candidate.enabled(currentContext))
        .flatMap((candidate) => candidate.keys)
        .map(shortcutSteps)
        .find((steps) => steps.length === 2 && matchesShortcut(event, steps[0]));
      if (!prefix) return;
      event.preventDefault();
      pendingStep.current = prefix[0].toLocaleLowerCase();
      pendingTimeout.current = window.setTimeout(clearPendingStep, 1000);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      clearPendingStep();
    };
  }, [execute]);
}

export function App() {
  const inboxSize = useInboxWidth();
  const {
    theme,
    effectiveTheme: effectiveThemeValue,
    setTheme,
    toggleTheme,
    fontScale,
    setFontScale,
    adjustFontScale,
    fontFamily,
    setFontFamily,
    autoReadDelaySeconds,
    setAutoReadDelaySeconds,
    loadRemoteImages,
    setLoadRemoteImages,
  } = useAppPreferences();
  const [threads, setThreads] = useState<Thread[]>([]);
  const [activeMessageFilters, setActiveMessageFilters] = useState<Set<MessageFilterKind>>(() => new Set());
  const toggleMessageFilter = useCallback((kind: MessageFilterKind) => {
    setActiveMessageFilters((current) => {
      const next = new Set(current);
      if (next.has(kind)) next.delete(kind);
      else next.add(kind);
      return next;
    });
  }, []);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checkedIds, setCheckedIds] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const visibleDetail = detail?.thread.id === selectedId ? detail : null;
  const selectedThread = threads.find((thread) => thread.id === selectedId);
  const selectedThreadLastMessageAt = selectedThread?.lastMessageAt;
  const selectedThreadSnippet = selectedThread?.snippet;
  const selectedThreadRowRef = useRef<HTMLButtonElement | null>(null);
  const autoReadSuppressedForId = useRef<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const {
    accounts,
    authStatus,
    setAuthStatus,
    syncStatus,
    setSyncStatus,
    activeAccountId,
    setActiveAccountId,
    mailboxUnreadCounts,
    refreshMailboxUnreadCounts,
    refreshAccounts,
    reorderAccounts,
  } = useAccounts(settingsOpen);
  const [unreadCounts, setUnreadCounts] = useState<UnreadCounts>({});
  const refreshUnreadCounts = useCallback(() => {
    void mailClient.listUnreadCounts().then(setUnreadCounts).catch(() => {});
  }, []);
  useEffect(refreshUnreadCounts, [refreshUnreadCounts]);
  const correspondence = useCorrespondence(accounts, visibleDetail?.messages.at(-1)?.id, visibleDetail?.thread.accountId);
  const composerBelongsToVisibleThread = Boolean(
    correspondence.activeDraft
    && correspondence.activeDraft.mode !== "new"
    && visibleDetail?.messages.some((message) => message.id === correspondence.activeDraft?.sourceId),
  );
  const displayedMessages = useMemo(
    () => visibleDetail ? messagesWithQueuedReplies(visibleDetail, correspondence.outbox) : [],
    [visibleDetail, correspondence.outbox],
  );
  const {
    messageExpansionOverrides,
    setMessageExpansionOverrides,
    latestMessageRef,
    messageStackRef,
    messageRefs,
    activeMessageIdRef,
    pendingMessageToggleFocusRef,
    activeMessageId,
    setActiveMessageId,
    latestDisplayedMessageId,
  } = useReaderState({
    selectedThreadId: selectedId,
    detail: visibleDetail,
    displayedMessages,
    composerOpen: composerBelongsToVisibleThread,
  });
  const [query, setQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [includeArchived, setIncludeArchived] = useState(false);
  const [hasMoreResults, setHasMoreResults] = useState(false);
  const [loading, setLoading] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false);
  const [unsubscribeMessageId, setUnsubscribeMessageId] = useState<string | null>(null);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [calendarOpen, setCalendarOpen] = useState(false);
  const [calendarAccounts, setCalendarAccounts] = useState<CalendarAccount[]>([]);
  const refreshCalendarAccounts = useCallback(async () => {
    const next = await mailClient.listCalendarAccounts();
    setCalendarAccounts(next);
    return next;
  }, []);
  useEffect(() => {
    void refreshCalendarAccounts().catch(() => {});
  }, [refreshCalendarAccounts]);
  const [recoveryStatus, setRecoveryStatus] = useState<RecoveryStatus | null>(null);
  useEffect(() => {
    // One-shot: this only ever reflects what happened during this app
    // launch's database open, so there's nothing to refresh later.
    void mailClient.recoveryStatus().then(setRecoveryStatus).catch(() => {});
  }, []);
  const [labelTargetIds, setLabelTargetIds] = useState<string[] | null>(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");
  const [lightboxImageSrc, setLightboxImageSrc] = useState<string | null>(null);
  const [aiSummaryAvailable, setAiSummaryAvailable] = useState(false);
  const [summaryExpanded, setSummaryExpanded] = useState(false);
  // Keyed by thread id, not a single flag, so summarizing thread A in the
  // background doesn't show "Summarizing…" (or clear it) on thread B just
  // because B is what's currently on screen when A's request settles.
  const summarizingRef = useRef<Set<string>>(new Set());
  const [summarizingIds, setSummarizingIds] = useState<Set<string>>(new Set());
  const [summaryErrors, setSummaryErrors] = useState<Record<string, string>>({});
  const refreshAiAvailability = useCallback(() => {
    const enabled = readAiProvider() !== "none" && readAiFeatures().summarize;
    if (!enabled) {
      setAiSummaryAvailable(false);
      return;
    }
    void isAiApiKeyConfigured()
      .then(setAiSummaryAvailable)
      .catch(() => setAiSummaryAvailable(false));
  }, []);
  useEffect(() => {
    // Also covers the initial mount, since `settingsOpen` starts `false`.
    if (!settingsOpen) refreshAiAvailability();
  }, [settingsOpen, refreshAiAvailability]);
  const [labels, setLabels] = useState<Label[]>([]);
  // Gmail user-label ids (for example `Label_18`) are only meaningful within
  // an account. Keep the catalogs separate so the same id in two accounts
  // cannot be displayed with the wrong account's label name.
  const [labelsByAccount, setLabelsByAccount] = useState<Record<string, Label[]>>({});
  const [splitInboxes, setSplitInboxes] = useState<SplitInbox[]>([]);
  const [splitInboxesLoaded, setSplitInboxesLoaded] = useState(false);
  const [activeSplitInboxId, setActiveSplitInboxId] = useState<string | null>(null);
  const refreshSplitInboxes = useCallback(() => {
    return mailClient.listSplitInboxes()
      .then((next) => setSplitInboxes(next))
      .catch(() => {})
      .finally(() => setSplitInboxesLoaded(true));
  }, []);
  useEffect(() => {
    void refreshSplitInboxes();
  }, [refreshSplitInboxes]);
  const [mailbox, setMailbox] = useState<MailboxKind>("inbox");
  const [mailboxError, setMailboxError] = useState("");
  const [detailLoading, setDetailLoading] = useState(false);
  const isThreadMailbox = mailbox === "inbox" || mailbox === "allMail" || mailbox === "trash" || mailbox === "split";
  const isTabbedMailbox = mailbox === "inbox" || mailbox === "split";
  // A split inbox belongs to one account, so the tab bar (and Tab/Shift+Tab
  // cycling) only ever offers the active account's own splits — never a
  // different account's, and none at all in the merged "All accounts" view.
  const accountSplitInboxes = useMemo(
    () => splitInboxes.filter((splitInbox) => splitInbox.accountId === activeAccountId),
    [splitInboxes, activeAccountId],
  );
  const activeSplitInbox = activeSplitInboxId
    ? accountSplitInboxes.find((candidate) => candidate.id === activeSplitInboxId) ?? null
    : null;
  const mailboxTitle = mailbox === "split" ? activeSplitInbox?.name ?? MAILBOX_TITLES.split : MAILBOX_TITLES[mailbox];
  useEffect(() => {
    // Deleting the split inbox currently being viewed (e.g. from Settings in
    // another render), or switching to an account it doesn't belong to,
    // shouldn't leave the thread list stuck on a rule that doesn't apply.
    if (activeSplitInboxId && splitInboxesLoaded && !activeSplitInbox) {
      setMailbox("inbox");
      setActiveSplitInboxId(null);
    }
  }, [activeSplitInboxId, activeSplitInbox, splitInboxesLoaded]);
  const restoredTabAccountRef = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    // Restores whichever Inbox/split tab this account last had selected —
    // waits for splitInboxesLoaded so a stored split id isn't mistaken for
    // deleted before the real list has a chance to arrive.
    if (!splitInboxesLoaded || restoredTabAccountRef.current === activeAccountId) return;
    restoredTabAccountRef.current = activeAccountId;
    const stored = readSelectedTabForAccount(activeAccountId);
    if (stored === undefined) return;
    const target = stored && accountSplitInboxes.some((candidate) => candidate.id === stored) ? stored : null;
    setMailbox((current) => (current === "inbox" || current === "split" ? (target ? "split" : "inbox") : current));
    setActiveSplitInboxId(target);
  }, [activeAccountId, splitInboxesLoaded, accountSplitInboxes]);
  const threadsRequest = useRef(0);
  const detailRequest = useRef(0);
  const triageSessionRef = useRef<TriageSession | null>(null);
  const triageCloseTimerRef = useRef<number | null>(null);
  const loadingMore = useRef(false);
  const [loadingMoreState, setLoadingMoreState] = useState(false);
  const [notice, setNotice] = useNotice();
  const recordTriageEvent = useCallback((event: TriageEvent) => {
    // Instrumentation is deliberately best-effort: a local telemetry write
    // must never make a mail action or navigation fail.
    void mailClient.recordTriageEvent(event).catch(() => {});
  }, []);
  const lastUndo = useRef<{ command: Command; result: CommandResult } | null>(null);
  const [canUndoAction, setCanUndoAction] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const selectAllRef = useRef<HTMLInputElement>(null);

  const undoLastAction = useCallback(async () => {
    const pending = lastUndo.current;
    if (!pending?.command.undo) return;
    lastUndo.current = null;
    setCanUndoAction(false);
    setNotice(null);
    try {
      await pending.command.undo(pending.result);
    } catch {
      setNotice({ message: "Undo could not be saved" });
    }
  }, [setNotice]);

  const loadThreads = useCallback(async (search: string, accountOverride?: string | null, mailboxOverride?: MailboxKind) => {
    const requestId = ++threadsRequest.current;
    const box = mailboxOverride ?? mailbox;
    if (box === "drafts" || box === "outbox" || (box === "split" && !activeSplitInboxId)) {
      setThreads([]);
      setSelectedId(null);
      setHasMoreResults(false);
      setMailboxError("");
      return;
    }
    setMailboxError("");
    const trimmed = search.trim();
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    try {
      const page = box === "inbox" && trimmed
        ? await mailClient.searchThreads({
            query: trimmed,
            limit: SEARCH_PAGE_SIZE,
            includeArchived,
          }, accountId).then((threads) => ({ threads, hasMore: threads.length === SEARCH_PAGE_SIZE }))
        : box === "allMail"
          ? await mailClient.listAllMailPage(accountId, 0, SEARCH_PAGE_SIZE)
          : box === "trash"
            ? await mailClient.listTrashPage(accountId, 0, SEARCH_PAGE_SIZE)
            : box === "split"
              ? await mailClient.listSplitInboxPage(activeSplitInboxId as string, 0, SEARCH_PAGE_SIZE)
              : await mailClient.listThreadsPage(accountId, 0, SEARCH_PAGE_SIZE);
      if (requestId !== threadsRequest.current) return;
      setMailboxError("");
      setThreads(page.threads);
      setHasMoreResults(page.hasMore);
      refreshUnreadCounts();
      refreshMailboxUnreadCounts();
      setSelectedId((current) =>
        current && page.threads.some((thread) => thread.id === current)
          ? current
          : (page.threads[0]?.id ?? null),
      );
    } catch (error) {
      if (requestId !== threadsRequest.current) return;
      setMailboxError(error instanceof Error ? error.message : String(error));
    }
  }, [includeArchived, activeAccountId, mailbox, activeSplitInboxId, refreshUnreadCounts, refreshMailboxUnreadCounts]);

  const loadMoreResults = useCallback(async () => {
    const trimmed = query.trim();
    if ((mailbox === "drafts" || mailbox === "outbox") || loadingMore.current) return;
    if (mailbox === "split" && !activeSplitInboxId) return;
    loadingMore.current = true;
    setLoadingMoreState(true);
    const requestId = threadsRequest.current;
    try {
      const accountId = activeAccountId ?? undefined;
      const page = trimmed
        ? await mailClient.searchThreads({
            query: trimmed,
            limit: SEARCH_PAGE_SIZE,
            offset: threads.length,
            includeArchived,
          }, accountId).then((items) => ({ threads: items, hasMore: items.length === SEARCH_PAGE_SIZE }))
        : mailbox === "allMail"
          ? await mailClient.listAllMailPage(accountId, threads.length, SEARCH_PAGE_SIZE)
          : mailbox === "trash"
            ? await mailClient.listTrashPage(accountId, threads.length, SEARCH_PAGE_SIZE)
            : mailbox === "split"
              ? await mailClient.listSplitInboxPage(activeSplitInboxId as string, threads.length, SEARCH_PAGE_SIZE)
              : await mailClient.listThreadsPage(accountId, threads.length, SEARCH_PAGE_SIZE);
      if (requestId !== threadsRequest.current) return;
      setThreads((current) => [...current, ...page.threads]);
      setHasMoreResults(page.hasMore);
    } catch (error) {
      if (requestId === threadsRequest.current) {
        setMailboxError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      loadingMore.current = false;
      setLoadingMoreState(false);
    }
  }, [query, threads.length, includeArchived, activeAccountId, mailbox, activeSplitInboxId]);

  useEffect(() => {
    if (correspondence.sentCount > 0) void loadThreads(query);
  }, [correspondence.sentCount, loadThreads]);

  useEffect(() => {
    void mailClient.listLabels().then(setLabels).catch(() => setLabels([]));
  }, []);

  useEffect(() => {
    const accountId = detail?.thread.accountId;
    if (!accountId || labelsByAccount[accountId]) return;
    let current = true;
    void mailClient
      .listLabels(accountId)
      .then((accountLabels) => {
        if (!current) return;
        setLabelsByAccount((catalogs) => ({ ...catalogs, [accountId]: accountLabels }));
      })
      .catch(() => {
        // Leave the catalog unset so selecting this account again can retry.
      });
    return () => {
      current = false;
    };
  }, [detail?.thread.accountId, labelsByAccount]);

  useEffect(() => {
    const requestId = ++detailRequest.current;
    if (!selectedId) {
      setDetail(null);
      setDetailLoading(false);
      return;
    }
    // Keep the existing conversation visible while refreshing it after a
    // send. Clear immediately only when the user selected a different thread.
    setDetail((current) => current?.thread.id === selectedId ? current : null);
    setDetailLoading(true);
    void mailClient.getThread(selectedId)
      .then((next) => {
        if (requestId !== detailRequest.current || next.thread.id !== selectedId) return;
        setDetail(next);
      })
      .catch((error) => {
        if (requestId !== detailRequest.current) return;
        setDetail(null);
        setNotice({ message: `Could not open conversation: ${error instanceof Error ? error.message : String(error)}` });
      })
      .finally(() => {
        if (requestId === detailRequest.current) setDetailLoading(false);
      });
  }, [selectedId, selectedThreadLastMessageAt, selectedThreadSnippet, setNotice]);

  useEffect(() => {
    setSummaryExpanded(false);
    // Pending/error state deliberately isn't reset here — it's keyed by
    // thread id (see `summarizingRef`/`summaryErrors`) so it stays correct
    // for whichever thread it actually belongs to when you navigate back.
  }, [selectedId]);

  useEffect(() => {
    if (!visibleDetail) return;
    const threadId = visibleDetail.thread.id;
    const context: TriageEvent["context"] = mailbox === "inbox" && !includeArchived ? "inbox" : "other";
    if (triageCloseTimerRef.current !== null) {
      window.clearTimeout(triageCloseTimerRef.current);
      triageCloseTimerRef.current = null;
    }
    const existing = triageSessionRef.current;
    if (existing && (existing.threadId !== threadId || existing.context !== context)) {
      recordTriageEvent(buildTriageCloseEvent(existing, triageNow()));
      triageSessionRef.current = null;
    }
    const session = triageSessionRef.current ?? {
      threadId,
      context,
      startedAt: triageNow(),
      activeElapsedMs: 0,
      active: true,
      scrolled: false,
    };
    if (triageSessionRef.current !== session) {
      triageSessionRef.current = session;
      recordTriageEvent({
        threadId: session.threadId,
        kind: "open",
        context: session.context,
      });
    }

    const messageStack = messageStackRef.current;
    const onScroll = () => {
      if (messageStack && messageStack.scrollTop > 8) session.scrolled = true;
    };
    const onPause = () => pauseTriageSession(session, triageNow());
    const onResume = () => resumeTriageSession(session, triageNow());
    const onVisibilityChange = () => {
      if (document.visibilityState === "hidden") onPause();
      else onResume();
    };
    messageStack?.addEventListener("scroll", onScroll, { passive: true });
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("blur", onPause);
    window.addEventListener("focus", onResume);
    return () => {
      messageStack?.removeEventListener("scroll", onScroll);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("blur", onPause);
      window.removeEventListener("focus", onResume);
      if (triageSessionRef.current !== session) return;
      // React Strict Mode replays effects immediately in development. Delay
      // the close one tick so the next setup can reuse the same session.
      triageCloseTimerRef.current = window.setTimeout(() => {
        triageCloseTimerRef.current = null;
        if (triageSessionRef.current !== session) return;
        recordTriageEvent(buildTriageCloseEvent(session, triageNow()));
        triageSessionRef.current = null;
      }, 0);
    };
  }, [includeArchived, mailbox, recordTriageEvent, visibleDetail?.thread.id]);

  const refreshMail = useCallback(() => {
    setSyncStatus((current) => current ? { ...current, state: "syncing" } : current);
    void mailClient.sync()
      .then((status) => {
        setSyncStatus(status);
        void loadThreads(query);
      })
      .catch(() => {
        void mailClient.syncStatus()
          .then((status) => setSyncStatus(status))
          .catch(() => {
            setSyncStatus((current) => current ? { ...current, state: "error" } : current);
          });
      });
  }, [loadThreads, query]);

  const refreshMailRef = useRef(refreshMail);
  refreshMailRef.current = refreshMail;

  useEffect(() => {
    let timer = 0;
    const flushIfInactive = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (document.visibilityState === "visible" && document.hasFocus()) return;
        void mailClient.flushPending().then(setSyncStatus).catch(() => {});
      }, 150);
    };
    const catchUp = createForegroundRefreshController(() => {
      refreshMailRef.current();
    });
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        catchUp.onBackground();
        flushIfInactive();
      } else {
        catchUp.onForeground();
      }
    };
    const onBlur = () => {
      catchUp.onBackground();
      flushIfInactive();
    };
    const onFocus = () => catchUp.onForeground();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("blur", onBlur);
    window.addEventListener("focus", onFocus);
    window.addEventListener("pageshow", onFocus);
    return () => {
      window.clearTimeout(timer);
      catchUp.dispose();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("pageshow", onFocus);
    };
  }, []);

  useEffect(() => {
    // Invalidate an in-flight request as soon as the view inputs change. The
    // debounce below is intentionally only for starting the replacement
    // request; it must not leave an older search eligible to paint.
    ++threadsRequest.current;
    const timeout = window.setTimeout(() => {
      void loadThreads(query).finally(() => setLoading(false));
    }, query.trim() ? 180 : 0);
    return () => window.clearTimeout(timeout);
  }, [query, loadThreads]);

  useEffect(() => {
    // The toggle itself is only reachable while searching; reset it with the
    // search box so a stale "include archived" flag can't linger over into
    // the plain inbox view, where the backend always excludes archived mail.
    if (!query.trim()) setIncludeArchived(false);
  }, [query]);

  useEffect(() => {
    if (searchOpen && mailbox === "inbox") searchRef.current?.focus();
  }, [mailbox, searchOpen]);

  useEffect(() => {
    setCheckedIds(new Set());
  }, [query, includeArchived]);

  useEffect(() => {
    setCheckedIds((current) => {
      if (current.size === 0) return current;
      const next = new Set([...current].filter((id) => threads.some((thread) => thread.id === id)));
      return next.size === current.size ? current : next;
    });
  }, [threads]);

  useEscapeDismiss(() => {
    setCheckedIds((current) => (current.size > 0 ? new Set() : current));
    setActiveMessageFilters((current) => (current.size > 0 ? new Set() : current));
  });

  const mutateIds = useCallback(async (ids: string[], template: MutationTemplate): Promise<CommandResult> => {
    const targetIds = ids.filter((id) => threads.some((thread) => thread.id === id));
    if (targetIds.length === 0) return {};
    if (template.kind === "read" && !template.value && selectedId && targetIds.includes(selectedId)) {
      autoReadSuppressedForId.current = selectedId;
    }
    const triageContext: TriageEvent["context"] = mailbox === "inbox" && !includeArchived ? "inbox" : "other";
    const triageEvents = template.kind === "archive" || template.kind === "trash"
      ? new Map(targetIds.map((threadId) => {
          const event = template.value
            ? buildTriageDispositionEvent({
                threadId,
                action: template.kind,
                context: triageContext,
                session: triageSessionRef.current,
                now: triageNow(),
                batch: targetIds.length > 1,
              })
            : {
                threadId,
                kind: "restore" as const,
                context: triageSessionRef.current?.threadId === threadId
                  ? triageSessionRef.current.context
                  : triageContext,
                action: template.kind,
                batch: targetIds.length > 1,
              } satisfies TriageEvent;
          return [threadId, event] as const;
        }))
      : null;
    const previous = new Map(
      threads.filter((thread) => targetIds.includes(thread.id)).map((thread) => [thread.id, thread] as const),
    );
    const previousDetail = detail && targetIds.includes(detail.thread.id) ? detail : null;
    // "split" intentionally falls into the default branch below: a split
    // inbox's underlying set is the same unarchived/untrashed inbox scope,
    // so archiving/trashing/spamming a thread should remove it exactly like
    // it would from the plain Inbox.
    const removesFromView = mailbox === "trash"
      ? template.kind === "trash" && !template.value
      : mailbox === "allMail"
        ? template.kind === "trash" && template.value
        : (template.kind === "archive" || template.kind === "trash" || template.kind === "spam")
            && template.value
            && !includeArchived;

    setThreads((current) => {
      const mapped = current.map((thread) =>
        previous.has(thread.id) ? applyMutationTemplate(thread, template) : thread,
      );
      return removesFromView ? mapped.filter((thread) => !targetIds.includes(thread.id)) : mapped;
    });
    setDetail((current) => {
      if (!current || !targetIds.includes(current.thread.id)) return current;
      const messages = template.kind === "read"
        ? current.messages.map((message, index) => ({
            ...message,
            unread: template.value ? false : index === current.messages.length - 1,
          }))
        : current.messages;
      return { ...current, thread: applyMutationTemplate(current.thread, template), messages };
    });

    if (removesFromView && selectedId && targetIds.includes(selectedId)) {
      const currentIndex = threads.findIndex((thread) => thread.id === selectedId);
      const remaining = threads.filter((thread) => !targetIds.includes(thread.id));
      const nextIndex = Math.min(currentIndex, remaining.length - 1);
      setSelectedId(remaining[nextIndex]?.id ?? null);
    }

    if (removesFromView) {
      setCheckedIds((current) => {
        if (current.size === 0) return current;
        const next = new Set(current);
        targetIds.forEach((id) => next.delete(id));
        return next.size === current.size ? current : next;
      });
    }

    let failedIds: string[] = [];
    try {
      await mailClient.mutateThreads(targetIds.map((id) => buildThreadMutation(id, template)));
    } catch {
      failedIds = targetIds;
    }
    const succeededIds = targetIds.filter((id) => !failedIds.includes(id));

    if (triageEvents) {
      succeededIds.forEach((id) => {
        const event = triageEvents.get(id);
        if (event) recordTriageEvent(event);
      });
    }

    if (failedIds.length > 0) {
      setThreads((current) => {
        const restored = failedIds
          .map((id) => previous.get(id))
          .filter((thread): thread is Thread => Boolean(thread));
        return sortByRecency([...current.filter((thread) => !failedIds.includes(thread.id)), ...restored]);
      });
      if (previousDetail && failedIds.includes(previousDetail.thread.id)) setDetail(previousDetail);
    }

    if (removesFromView) {
      await loadThreads(query);
    }
    if (document.visibilityState !== "visible" || !document.hasFocus()) {
      void mailClient.flushPending().then(setSyncStatus).catch(() => {});
    }

    if (succeededIds.length === 0) {
      setNotice({ message: "Change could not be saved" });
      return {};
    }

    const labelName = template.kind === "label"
      ? labels.find((label) => label.id === template.labelId)?.name
      : undefined;
    const definite = describeMutation(template, succeededIds.length, labelName);
    const singleToggle = (template.kind === "star" || template.kind === "read")
      && succeededIds.length === 1 && failedIds.length === 0;
    const message = singleToggle
      ? undefined
      : failedIds.length > 0
        ? `${definite} — ${failedIds.length} could not be saved`
        : definite;
    const undoTemplate = invertMutationTemplate(template);

    return {
      message,
      undoAction: async () => {
        setThreads((current) => {
          const restored = succeededIds
            .map((id) => previous.get(id))
            .filter((thread): thread is Thread => Boolean(thread));
          return sortByRecency([...current.filter((thread) => !succeededIds.includes(thread.id)), ...restored]);
        });
        if (removesFromView && succeededIds.length === 1) setSelectedId(succeededIds[0]);
        await mailClient.mutateThreads(succeededIds.map((id) => buildThreadMutation(id, undoTemplate)));
        if (triageEvents) {
          succeededIds.forEach((id) => {
            const event = triageEvents.get(id);
            if (!event || event.kind !== "disposition") return;
            recordTriageEvent({
              threadId: id,
              kind: "restore",
              context: event.context,
              action: event.action,
              batch: event.batch,
            });
          });
        }
        await loadThreads(query);
      },
    };
  }, [threads, detail, includeArchived, mailbox, selectedId, loadThreads, query, labels, recordTriageEvent, setNotice]);

  const visibleThreads = useMemo(
    () => filterThreadsByMessageFilters(threads, activeMessageFilters),
    [threads, activeMessageFilters],
  );
  const selected = threads.find((thread) => thread.id === selectedId) ?? null;
  const selectedIndex = visibleThreads.findIndex((thread) => thread.id === selectedId);
  const accountColors = useMemo(
    () => new Map(accounts.map((account) => [account.email, account.color] as const)),
    [accounts],
  );
  const selectThread = useCallback((id: string) => setSelectedId(id), []);
  const toggleChecked = useCallback((id: string) => {
    setCheckedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const latestMessage = visibleDetail?.messages.at(-1) ?? null;
  const canUnsubscribe = Boolean(latestMessage?.unsubscribe?.methods.length);
  const unsubscribeMessage = visibleDetail?.messages.find((message) => message.id === unsubscribeMessageId) ?? null;
  const summaryPending = visibleDetail ? summarizingIds.has(visibleDetail.thread.id) : false;
  const summaryError = visibleDetail ? summaryErrors[visibleDetail.thread.id] ?? null : null;

  const mutateIdsRef = useRef(mutateIds);
  mutateIdsRef.current = mutateIds;

  // Explicitly marking the open thread unread suppresses auto-read until the
  // selection changes. The timer itself is not recorded here, so changing the
  // configured delay can cancel and reschedule it with the new duration.
  useEffect(() => {
    autoReadSuppressedForId.current = null;
  }, [selectedId]);

  useEffect(() => {
    if (
      !selectedId
      || visibleDetail?.thread.id !== selectedId
      || !isThreadMailbox
      || autoReadSuppressedForId.current === selectedId
    ) return;
    if (!selected?.unread) return;

    const timer = window.setTimeout(() => {
      void mutateIdsRef.current([selectedId], { kind: "read", value: true });
    }, autoReadDelaySeconds * 1000);
    return () => window.clearTimeout(timer);
  }, [autoReadDelaySeconds, isThreadMailbox, selected?.unread, selectedId, visibleDetail?.thread.id]);

  const confirmUnsubscribe = useCallback(async () => {
    if (!unsubscribeMessageId) return;
    try {
      const result = await mailClient.unsubscribe(unsubscribeMessageId);
      setUnsubscribeMessageId(null);
      setNotice({
        message: result.outcome === "requested"
          ? "Unsubscribe request sent"
          : "Opened unsubscribe option",
      });
    } catch (reason: unknown) {
      setNotice({
        message: `Unsubscribe failed: ${reason instanceof Error ? reason.message : String(reason)}`,
      });
    }
  }, [setNotice, unsubscribeMessageId]);

  const openSettingsAt = useCallback((section: SettingsSection) => {
    setSettingsSection(section);
    setSettingsOpen(true);
  }, []);

  const openToday = useCallback(() => {
    void refreshCalendarAccounts()
      .then((connected) => {
        if (!connected.some((account) => account.status === "connected")) {
          openSettingsAt("calendarAccounts");
          return;
        }
        setCalendarOpen(true);
      })
      .catch((reason: unknown) => {
        setNotice({ message: reason instanceof Error ? reason.message : String(reason) });
      });
  }, [openSettingsAt, refreshCalendarAccounts, setNotice]);

  /**
   * Always calls the provider, even when a summary is already cached — used
   * for both the first generation and an explicit "Regenerate". Guarded by
   * `summarizingRef` (checked and updated synchronously, not via state) so
   * pressing "i" or Regenerate repeatedly for the same thread while a
   * request is already in flight doesn't fire duplicate provider calls; a
   * different thread can still summarize concurrently in the background.
   */
  const runSummarize = useCallback(async () => {
    if (!selected) return;
    const threadId = selected.id;
    if (summarizingRef.current.has(threadId)) return;
    summarizingRef.current.add(threadId);
    setSummarizingIds(new Set(summarizingRef.current));
    setSummaryExpanded(true);
    setSummaryErrors((current) => {
      if (!(threadId in current)) return current;
      const next = { ...current };
      delete next[threadId];
      return next;
    });
    try {
      const provider = readAiProvider();
      const model = resolveAiModel(provider, readAiModel());
      if (!model) throw new Error("Set a model in AI settings before summarizing.");
      const endpoint = provider === "custom" ? readAiEndpoint().trim() : null;
      if (provider === "custom" && !endpoint) {
        throw new Error("Set an endpoint URL in AI settings before summarizing.");
      }
      const result = await mailClient.summarizeThread(threadId, provider, model, endpoint);
      setThreads((current) =>
        current.map((thread) =>
          thread.id === threadId
            ? { ...thread, summary: result.summary, summaryGeneratedAt: result.generatedAt }
            : thread,
        ),
      );
      setDetail((current) =>
        current && current.thread.id === threadId
          ? {
              ...current,
              thread: { ...current.thread, summary: result.summary, summaryGeneratedAt: result.generatedAt },
            }
          : current,
      );
    } catch (error) {
      setSummaryErrors((current) => ({
        ...current,
        [threadId]: error instanceof Error ? error.message : String(error),
      }));
    } finally {
      summarizingRef.current.delete(threadId);
      setSummarizingIds(new Set(summarizingRef.current));
    }
  }, [selected]);

  useEffect(() => {
    selectedThreadRowRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [selectedId]);

  const selectAdjacentMessage = useCallback((direction: -1 | 1) => {
    if (displayedMessages.length === 0) return;
    const activeIndex = displayedMessages.findIndex((message) => message.id === activeMessageIdRef.current);
    const currentIndex = activeIndex >= 0 ? activeIndex : displayedMessages.length - 1;
    const targetIndex = Math.max(0, Math.min(displayedMessages.length - 1, currentIndex + direction));
    const target = displayedMessages[targetIndex];
    if (!target) return;

    activeMessageIdRef.current = target.id;
    const node = messageRefs.current.get(target.id);
    if (!node) return;
    const focusTarget = node.querySelector<HTMLElement>(".message-card-toggle, .message-expanded-toggle") ?? node;
    focusTarget.focus({ preventScroll: true });
    node.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
  }, [displayedMessages]);

  const goToInboxTab = useCallback(() => {
    correspondence.context.openInbox();
    setQuery("");
    setSearchOpen(false);
    setMailbox("inbox");
    setActiveSplitInboxId(null);
    saveSelectedTabForAccount(activeAccountId, null);
  }, [correspondence.context, activeAccountId]);

  const goToSplitTab = useCallback((id: string) => {
    correspondence.context.openInbox();
    setQuery("");
    setSearchOpen(false);
    setMailbox("split");
    setActiveSplitInboxId(id);
    saveSelectedTabForAccount(activeAccountId, id);
  }, [correspondence.context, activeAccountId]);

  // Cycles through Inbox + every split inbox tab, in the order the tab bar
  // shows them, wrapping around at either end.
  const goToRelativeSplitTab = useCallback((direction: 1 | -1) => {
    const tabs: (string | null)[] = [null, ...accountSplitInboxes.map((splitInbox) => splitInbox.id)];
    const currentIndex = mailbox === "split" ? tabs.indexOf(activeSplitInboxId) : 0;
    const from = currentIndex === -1 ? 0 : currentIndex;
    const target = tabs[(from + direction + tabs.length) % tabs.length];
    if (target === null) goToInboxTab();
    else goToSplitTab(target);
  }, [accountSplitInboxes, mailbox, activeSplitInboxId, goToInboxTab, goToSplitTab]);
  const goToNextSplitTab = useCallback(() => goToRelativeSplitTab(1), [goToRelativeSplitTab]);
  const goToPreviousSplitTab = useCallback(() => goToRelativeSplitTab(-1), [goToRelativeSplitTab]);

  const context = useMemo<CommandContext>(() => ({
    ...correspondence.context,
    mailbox,
    selectedId,
    selectedArchived: selected?.archived ?? false,
    selectedTrashed: selected?.trashed ?? false,
    canUnsubscribe,
    canNavigateMessages: displayedMessages.length > 1,
    canSendAndMarkDone: composerBelongsToVisibleThread,
    sendAndMarkDone: () => {
      if (!selected || !composerBelongsToVisibleThread) return;
      const threadId = selected.id;
      correspondence.context.sendDraftAndThen(() => {
        void mutateIds([threadId], { kind: "archive", value: true });
      });
    },
    openInbox: goToInboxTab,
    splitInboxCount: accountSplitInboxes.length,
    goToNextSplitTab,
    goToPreviousSplitTab,
    openAllMail: () => {
      correspondence.context.openInbox();
      setQuery("");
      setSearchOpen(false);
      setMailbox("allMail");
    },
    openTrash: () => {
      correspondence.context.openInbox();
      setQuery("");
      setSearchOpen(false);
      setMailbox("trash");
    },
    openSplitInbox: goToSplitTab,
    openDrafts: () => {
      correspondence.context.openDrafts();
      setQuery("");
      setSearchOpen(false);
      setMailbox("drafts");
      setSelectedId(null);
      setDetail(null);
    },
    openOutbox: () => {
      correspondence.context.openOutbox();
      setQuery("");
      setSearchOpen(false);
      setMailbox("outbox");
      setSelectedId(null);
      setDetail(null);
    },
    selectNext: () => {
      const next = Math.min(selectedIndex + 1, visibleThreads.length - 1);
      setSelectedId(visibleThreads[next]?.id ?? null);
    },
    selectPrevious: () => {
      const next = Math.max(selectedIndex - 1, 0);
      setSelectedId(visibleThreads[next]?.id ?? null);
    },
    selectNextMessage: () => selectAdjacentMessage(1),
    selectPreviousMessage: () => selectAdjacentMessage(-1),
    archiveSelected: () => mutateIds(selected ? [selected.id] : [], { kind: "archive", value: true }),
    markNotDoneSelected: () => mutateIds(selected ? [selected.id] : [], { kind: "archive", value: false }),
    unsubscribeSelected: () => {
      if (latestMessage?.unsubscribe?.methods.length) setUnsubscribeMessageId(latestMessage.id);
    },
    trashSelected: () => mutateIds(selected ? [selected.id] : [], { kind: "trash", value: true }),
    restoreSelected: async () => {
      if (!selected) return {};
      // Matches Gmail: restoring from Trash always lands back in the Inbox,
      // regardless of whether the thread was archived before it was trashed.
      const wasArchived = selected.archived;
      const result = await mutateIds([selected.id], { kind: "trash", value: false });
      if (wasArchived) await mutateIds([selected.id], { kind: "archive", value: false });
      return result;
    },
    markSpamSelected: () => mutateIds(selected ? [selected.id] : [], { kind: "spam", value: true }),
    setLabelSelected: (labelId, value) => mutateIds(labelTargetIds ?? [], { kind: "label", labelId, value }),
    toggleReadSelected: () =>
      mutateIds(selected ? [selected.id] : [], { kind: "read", value: selected?.unread ?? false }),
    toggleStarSelected: () =>
      mutateIds(selected ? [selected.id] : [], { kind: "star", value: !(selected?.starred ?? true) }),
    reply: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      correspondence.context.reply();
    },
    replyAll: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      correspondence.context.replyAll();
    },
    forward: () => {
      if (selected) {
        recordTriageEvent({
          threadId: selected.id,
          kind: "response",
          context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
        });
      }
      correspondence.context.forward();
    },
    toggleCheckedSelected: () => {
      if (!selected) return;
      setCheckedIds((current) => {
        const next = new Set(current);
        if (next.has(selected.id)) next.delete(selected.id);
        else next.add(selected.id);
        return next;
      });
    },
    toggleOlderMessagesExpanded: () => setMessageExpansionOverrides((current) => {
      const olderMessages = displayedMessages.slice(0, -1);
      const allExpanded = olderMessages.every((message) => current.get(message.id) ?? message.unread);
      return new Map(olderMessages.map((message) => [message.id, !allExpanded]));
    }),
    pageMessageDown: () => {
      const node = messageStackRef.current;
      if (!node) return;
      node.scrollBy({ top: node.clientHeight * 0.9, behavior: "smooth" });
    },
    pageMessageUp: () => {
      const node = messageStackRef.current;
      if (!node) return;
      node.scrollBy({ top: -node.clientHeight * 0.9, behavior: "smooth" });
    },
    aiSummaryAvailable,
    summarizeSelected: async () => {
      if (!selected) return {};
      const cached = visibleDetail?.thread.id === selected.id ? visibleDetail.thread : null;
      if (cached?.summary) {
        setSummaryExpanded((current) => !current);
        return {};
      }
      await runSummarize();
      return {};
    },
    focusSearch: () => {
      correspondence.context.openInbox();
      setMailbox("inbox");
      setSearchOpen(true);
    },
    refresh: refreshMail,
    openDiagnostics: () => setDiagnosticsOpen(true),
    openLabels: () => setLabelTargetIds(selected ? [selected.id] : null),
    openPalette: () => setPaletteOpen(true),
    openShortcutHelp: () => setShortcutHelpOpen(true),
    openSettings: () => openSettingsAt("appearance"),
    openToday,
    increaseFontSize: () => adjustFontScale(1),
    decreaseFontSize: () => adjustFontScale(-1),
    canUndoAction,
    undoLastAction: () => { void undoLastAction(); },
    switchAccount: (email) => {
      setActiveAccountId(email);
    },
    showAllAccounts: () => {
      setActiveAccountId(null);
    },
    toggleMessageFilter,
  }), [accountSplitInboxes.length, adjustFontScale, aiSummaryAvailable, canUnsubscribe, canUndoAction, composerBelongsToVisibleThread, displayedMessages, goToInboxTab, goToNextSplitTab, goToPreviousSplitTab, goToSplitTab, includeArchived, labelTargetIds, latestMessage, mailbox, mutateIds, openSettingsAt, openToday, recordTriageEvent, refreshMail, runSummarize, selectAdjacentMessage, selected, selectedId, selectedIndex, toggleMessageFilter, visibleThreads, correspondence.context, undoLastAction, visibleDetail]);

  const executeCommand = useCallback((command: Command) => {
    void command.run(context)
      .then((result) => {
        if (!command.undo || !result.undoAction) return;
        lastUndo.current = { command, result };
        setCanUndoAction(true);
        setNotice({
          message: result.message ?? command.title,
          undo: () => { void undoLastAction(); },
        });
      })
      .catch((error: unknown) => {
        setNotice({ message: error instanceof Error ? error.message : String(error) });
      });
  }, [context, setNotice, undoLastAction]);
  const executeById = useCallback((id: string) => {
    const command = commands.find((candidate) => candidate.id === id);
    if (command?.enabled(context)) executeCommand(command);
  }, [context, executeCommand]);
  const accountCommands = useMemo<Command[]>(
    () =>
      accounts.length > 1
        ? [showAllAccountsCommand(), ...accounts.map((account, index) => accountCommand(account.email, index))]
        : [],
    [accounts],
  );
  const splitInboxCommands = useMemo<Command[]>(
    () => accountSplitInboxes.map((splitInbox) => splitInboxCommand(splitInbox)),
    [accountSplitInboxes],
  );
  const paletteExtraCommands = useMemo<Command[]>(
    () => [...accountCommands, ...splitInboxCommands],
    [accountCommands, splitInboxCommands],
  );
  useShortcutHandler(context, executeCommand, paletteExtraCommands);

  const runOnSelection = useCallback((title: string, template: MutationTemplate) => {
    executeCommand({
      id: "selection.batch",
      title,
      keys: [],
      group: "Triage",
      enabled: () => true,
      run: () => mutateIds([...checkedIds], template),
      undo: undoResult,
    });
  }, [checkedIds, executeCommand, mutateIds]);

  const reorderNavbarAccounts = useCallback((emails: string[]) => {
    void reorderAccounts(emails).catch(() => {
      setNotice({ message: "Account order could not be saved" });
    });
  }, [reorderAccounts, setNotice]);

  useEffect(() => {
    if (!selectAllRef.current) return;
    selectAllRef.current.indeterminate = checkedIds.size > 0 && checkedIds.size < threads.length;
  }, [checkedIds, threads.length]);

  const activeAccount = accounts.find((account) => account.email === activeAccountId) ?? null;

  const selectedThreads = threads.filter((thread) => checkedIds.has(thread.id));
  const allSelectedThreadsStarred = selectedThreads.length > 0
    && selectedThreads.every((thread) => thread.starred);
  const batchStarLabel = allSelectedThreadsStarred ? "Unstar" : "Star";

  return (
    <main className={`app-shell${calendarOpen ? " calendar-open" : ""}`} style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
      <nav className="sidebar" aria-label="Mailboxes">
        <AccountSwitcher
          accounts={accounts}
          unreadCounts={unreadCounts}
          activeAccountId={activeAccountId}
          onSwitch={context.switchAccount}
          onShowAll={context.showAllAccounts}
          onReorder={reorderNavbarAccounts}
        />
        <div className="sidebar-nav">
          <button className="nav-button" aria-label="New Message (c)" title="New Message (c)" onClick={() => executeById("draft.new")}><Pencil size={19} /></button>
          <HoverTooltip label="Inbox" shortcut="G I">
            <button
              className={`nav-button ${mailbox === "inbox" ? "active" : ""}`}
              aria-label="Inbox (g then i)"
              onClick={() => executeById("mailbox.inbox")}
            >
              <Inbox size={19} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="All Mail" shortcut="G A">
            <button
              className={`nav-button ${mailbox === "allMail" ? "active" : ""}`}
              aria-label="All Mail (g then a)"
              onClick={() => executeById("mailbox.allMail")}
            >
              <Mails size={19} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="Drafts" shortcut="G D">
            <button
              className={`nav-button ${mailbox === "drafts" ? "active" : ""}`}
              aria-label={`Drafts (${correspondence.draftCount}) (g then d)`}
              onClick={() => executeById("drafts.open")}
            >
              <FileText size={19} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="Outbox">
            <button
              className={`nav-button ${mailbox === "outbox" ? "active" : ""}`}
              aria-label={`Outbox (${correspondence.outboxCount})`}
              onClick={() => executeById("outbox.open")}
            >
              <Send size={19} />
            </button>
          </HoverTooltip>
          <HoverTooltip label="Trash" shortcut="G T">
            <button
              className={`nav-button ${mailbox === "trash" ? "active" : ""}`}
              aria-label="Trash (g then t)"
              onClick={() => executeById("mailbox.trash")}
            >
              <Trash2 size={19} />
            </button>
          </HoverTooltip>
        </div>
        <div className="sidebar-spacer" />
        <div className="sidebar-nav">
          <HoverTooltip label="Today’s schedule" shortcut="T">
            <button
              className={`nav-button ${calendarOpen ? "active" : ""}`}
              aria-label="Today’s schedule (T)"
              onClick={() => executeById("calendar.today")}
            >
              <CalendarDays size={19} />
            </button>
          </HoverTooltip>
          <button
            className="nav-button"
            aria-label="Refresh mail"
            title="Refresh mail"
            onClick={() => executeById("mail.refresh")}
          >
            <RefreshCw size={19} className={syncStatus?.state === "syncing" ? "spin" : ""} />
          </button>
          {(syncStatus?.failedMutations?.length ?? 0)
            + (syncStatus?.quarantinedMessages?.length ?? 0) > 0 ? (
            <button
              className="nav-button mutation-failure-button"
              aria-label="Sync diagnostics need attention"
              title="Sync diagnostics need attention"
              onClick={() => setDiagnosticsOpen(true)}
            >
              <AlertCircle size={19} />
            </button>
          ) : null}
          <button
            className="nav-button"
            aria-label={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`}
            title={`Switch to ${effectiveThemeValue === "dark" ? "light" : "dark"} mode`}
            onClick={toggleTheme}
          >
            {effectiveThemeValue === "dark" ? <Sun size={19} /> : <Moon size={19} />}
          </button>
          <button
            className="nav-button"
            aria-label="Keyboard shortcuts (?)"
            title="Keyboard shortcuts (?)"
            onClick={() => executeById("shortcuts.open")}
          >
            <Keyboard size={19} />
          </button>
          <button
            className="nav-button"
            aria-label="Command palette"
            onClick={() => executeById("palette.open")}
          >
            <CommandIcon size={19} />
          </button>
          <button
            className="nav-button"
            aria-label="Settings (⌘,)"
            title="Settings (⌘,)"
            onClick={() => executeById("settings.open")}
          >
            <SettingsIcon size={19} />
          </button>
        </div>
      </nav>

      <section id="inbox-panel" className="thread-column" aria-label="Inbox">
        <InboxResizeHandle {...inboxSize} />
        <header className="thread-header">
          <div className="thread-header-title">
            {isThreadMailbox && checkedIds.size > 0 ? (
              <label className="select-all">
                <input
                  ref={selectAllRef}
                  type="checkbox"
                  checked={threads.length > 0 && checkedIds.size === threads.length}
                  aria-label="Select all conversations"
                  onChange={(event) =>
                    setCheckedIds(event.target.checked ? new Set(threads.map((thread) => thread.id)) : new Set())
                  }
                />
              </label>
            ) : null}
            <div>
              <span className="eyebrow">
                {isTabbedMailbox ? null : mailboxTitle}
                {activeAccount ? (
                  <span className="eyebrow-account">{isTabbedMailbox ? "" : " · "}{activeAccount.email}</span>
                ) : null}
              </span>
              <h1>
                {mailbox === "drafts"
                  ? `${correspondence.drafts.length} drafts`
                  : mailbox === "outbox"
                    ? `${correspondence.outbox.filter((item) => item.state !== "canceled").length} outgoing`
                    : `${visibleThreads.length} conversations`}
              </h1>
              {isTabbedMailbox ? (
                <div className="mailbox-tabs">
                  <div className="mailbox-tab-list" role="tablist" aria-label="Mailbox views">
                    <button
                      type="button"
                      role="tab"
                      aria-selected={mailbox === "inbox"}
                      className={`mailbox-tab ${mailbox === "inbox" ? "active" : ""}`}
                      onClick={() => context.openInbox()}
                    >
                      Inbox{mailboxUnreadCounts.inbox > 0 ? ` ${mailboxUnreadCounts.inbox}` : ""}
                    </button>
                    {accountSplitInboxes.map((splitInbox) => (
                      <button
                        key={splitInbox.id}
                        type="button"
                        role="tab"
                        aria-selected={mailbox === "split" && activeSplitInboxId === splitInbox.id}
                        className={`mailbox-tab ${mailbox === "split" && activeSplitInboxId === splitInbox.id ? "active" : ""}`}
                        onClick={() => context.openSplitInbox(splitInbox.id)}
                      >
                        {splitInbox.name}
                        {mailboxUnreadCounts.splits[splitInbox.id] ? ` ${mailboxUnreadCounts.splits[splitInbox.id]}` : ""}
                      </button>
                    ))}
                  </div>
                  <button type="button" className="mailbox-tab-add" onClick={() => openSettingsAt("splitInboxes")}>
                    + Add Split
                  </button>
                </div>
              ) : null}
            </div>
          </div>
          {isThreadMailbox ? (
            <FiltersButton activeFilters={activeMessageFilters} onToggleFilter={toggleMessageFilter} />
          ) : null}
        </header>
        {isThreadMailbox && checkedIds.size > 0 ? (
          <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
            <span className="batch-count">{checkedIds.size} selected</span>
            <div className="batch-actions">
              {mailbox === "trash" ? (
                <HoverTooltip label="Restore" placement="bottom">
                  <ActionButton label="Restore" onClick={() => runOnSelection("Restore", { kind: "trash", value: false })}>
                    <RotateCcw size={16} />
                  </ActionButton>
                </HoverTooltip>
              ) : (
                <>
                  <HoverTooltip label="Archive" placement="bottom">
                    <ActionButton label="Archive" onClick={() => runOnSelection("Archive", { kind: "archive", value: true })}>
                      <Archive size={16} />
                    </ActionButton>
                  </HoverTooltip>
                  <HoverTooltip label="Trash" placement="bottom">
                    <ActionButton label="Trash" onClick={() => runOnSelection("Trash", { kind: "trash", value: true })}>
                      <Trash2 size={16} />
                    </ActionButton>
                  </HoverTooltip>
                  <HoverTooltip label="Mark spam" placement="bottom">
                    <ActionButton label="Mark spam" onClick={() => runOnSelection("Mark spam", { kind: "spam", value: true })}>
                      <ShieldAlert size={16} />
                    </ActionButton>
                  </HoverTooltip>
                </>
              )}
              <HoverTooltip label="Mark read" placement="bottom">
                <ActionButton label="Mark read" onClick={() => runOnSelection("Mark read", { kind: "read", value: true })}>
                  <MailOpen size={16} />
                </ActionButton>
              </HoverTooltip>
              <HoverTooltip label="Mark unread" placement="bottom">
                <ActionButton label="Mark unread" onClick={() => runOnSelection("Mark unread", { kind: "read", value: false })}>
                  <Mail size={16} />
                </ActionButton>
              </HoverTooltip>
              <HoverTooltip label={batchStarLabel} placement="bottom">
                <ActionButton
                  label={batchStarLabel}
                  onClick={() => runOnSelection(batchStarLabel, { kind: "star", value: !allSelectedThreadsStarred })}
                >
                  <Star size={16} fill={allSelectedThreadsStarred ? "currentColor" : "none"} />
                </ActionButton>
              </HoverTooltip>
              <HoverTooltip label="Labels" placement="bottom">
                <ActionButton label="Labels" onClick={() => setLabelTargetIds([...checkedIds])}>
                  <Tag size={16} />
                </ActionButton>
              </HoverTooltip>
              <HoverTooltip label="Clear selection" placement="bottom">
                <button
                  className="icon-button"
                  aria-label="Clear selection"
                  onClick={() => setCheckedIds(new Set())}
                >
                  <X size={16} />
                </button>
              </HoverTooltip>
            </div>
          </div>
        ) : null}
        {mailbox === "inbox" && searchOpen ? (
          <div className="list-toolbar">
            <label className="search-box">
              <Search size={16} />
              <input
                ref={searchRef}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search mail"
                aria-label="Search mail"
                onKeyDown={(event) => {
                  if (event.key !== "Escape") return;
                  event.preventDefault();
                  event.stopPropagation();
                  setQuery("");
                  setSearchOpen(false);
                  selectedThreadRowRef.current?.focus();
                }}
              />
              {query.trim() ? (
                <button
                  type="button"
                  className={`search-toggle ${includeArchived ? "active" : ""}`}
                  aria-pressed={includeArchived}
                  aria-label="Include archived or trashed mail in search"
                  title="Include archived or trashed mail in search"
                  onClick={() => setIncludeArchived((current) => !current)}
                >
                  <Archive size={14} />
                </button>
              ) : null}
              <kbd>/</kbd>
            </label>
          </div>
        ) : null}
        <div className="thread-list" role={isThreadMailbox ? "listbox" : "list"} aria-label={mailboxTitle}>
          {mailbox === "drafts" ? (
            <DraftsList drafts={correspondence.drafts} onOpen={correspondence.openDraft} />
          ) : mailbox === "outbox" ? (
            <OutboxList
              outbox={correspondence.outbox}
              clock={correspondence.clock}
              onUndo={correspondence.undoSendItem}
              onRestore={correspondence.restoreFailedSend}
              onReconcile={correspondence.reconcileSend}
              pendingActions={correspondence.pendingOutboxActions}
            />
          ) : (
            <>
          {loading ? <p className="empty">Loading inbox…</p> : null}
          {mailboxError ? <p className="empty mailbox-error" role="alert">{mailboxError}</p> : null}
          {!loading && threads.length === 0 ? (
            accounts.length === 0 ? (
              <div className="connect-account-cta">
                <Mail size={28} />
                <p>Connect your Gmail account to start syncing mail.</p>
                <button type="button" onClick={() => openSettingsAt("accounts")}>
                  Add account
                </button>
              </div>
            ) : (
              <p className="empty">{mailbox === "trash" ? "No trashed messages." : "Inbox zero."}</p>
            )
          ) : null}
          {!loading && threads.length > 0 && visibleThreads.length === 0 ? (
            <p className="empty">No conversations match the selected filters.</p>
          ) : null}
          {visibleThreads.map((thread) => (
            <ThreadRow
              key={thread.id}
              thread={thread}
              selected={thread.id === selectedId}
              checked={checkedIds.has(thread.id)}
              showAccount={accounts.length > 1}
              accountColor={accountColors.get(thread.accountId)}
              onSelect={selectThread}
              onToggleCheck={toggleChecked}
              rowRef={thread.id === selectedId ? selectedThreadRowRef : undefined}
            />
          ))}
          {hasMoreResults ? (
            <button className="load-more" onClick={() => void loadMoreResults()} disabled={loadingMoreState}>
              {loadingMoreState ? "Loading…" : "Load more results"}
            </button>
          ) : null}
            </>
          )}
        </div>
      </section>

      <section className="reader" aria-label="Conversation">
        {correspondence.activeDraft && !composerBelongsToVisibleThread ? (
          <div className="message-stack draft-message-stack">
            {correspondence.composer}
          </div>
        ) : visibleDetail ? (
          <>
            <header className="reader-header">
              <div>
                <span className="eyebrow">
                  {labelIdsForConversationDisplay(visibleDetail.thread.labels)
                    .map((id) => {
                      const accountLabels = labelsByAccount[visibleDetail.thread.accountId];
                      const label = accountLabels?.find((candidate) => candidate.id === id);
                      // Opaque Gmail user-label ids are not useful UI. Wait for
                      // the account catalog instead of briefly flashing Label_18.
                      if (!label && /^label_\d+$/i.test(id)) return null;
                      return label ? formatLabelName(label) : id;
                    })
                    .filter((label): label is string => label !== null)
                    .join(" · ")}
                </span>
                <h2>{visibleDetail.thread.subject}</h2>
              </div>
              <div className="reader-actions">
                <HoverTooltip label={selected?.starred ? "Unstar" : "Star"} shortcut="s" placement="bottom">
                  <ActionButton
                    label={selected?.starred ? "Unstar" : "Star"}
                    shortcut="s"
                    onClick={() => executeById("thread.star")}
                  >
                    <Star size={17} fill={selected?.starred ? "currentColor" : "none"} />
                  </ActionButton>
                </HoverTooltip>
                <HoverTooltip
                  label={selected?.unread ? "Mark read" : "Mark unread"}
                  shortcut="u"
                  placement="bottom"
                >
                  <ActionButton
                    label={selected?.unread ? "Mark read" : "Mark unread"}
                    shortcut="u"
                    onClick={() => executeById("thread.read")}
                  >
                    {selected?.unread ? <MailOpen size={17} /> : <Mail size={17} />}
                  </ActionButton>
                </HoverTooltip>
                {canUnsubscribe ? (
                  <HoverTooltip label="Unsubscribe" shortcut="⌘U" placement="bottom">
                    <ActionButton label="Unsubscribe" shortcut="⌘U" onClick={() => executeById("thread.unsubscribe")}>
                      <Unlink size={17} />
                    </ActionButton>
                  </HoverTooltip>
                ) : null}
                <HoverTooltip label="Manage Labels" shortcut="L" placement="bottom">
                  <ActionButton label="Labels" shortcut="l" onClick={() => executeById("labels.open")}>
                    <Tag size={17} />
                  </ActionButton>
                </HoverTooltip>
                {mailbox === "trash" ? null : selected?.archived ? (
                  <HoverTooltip label="Mark not done" shortcut="Shift+E" placement="bottom">
                    <ActionButton label="Mark not done" shortcut="Shift+E" onClick={() => executeById("thread.unarchive")}>
                      <Inbox size={17} />
                    </ActionButton>
                  </HoverTooltip>
                ) : (
                  <HoverTooltip label="Archive" shortcut="e" placement="bottom">
                    <ActionButton label="Archive" shortcut="e" onClick={() => executeById("thread.archive")}>
                      <Archive size={17} />
                    </ActionButton>
                  </HoverTooltip>
                )}
                {mailbox === "trash" ? (
                  <HoverTooltip label="Restore" placement="bottom">
                    <ActionButton label="Restore" onClick={() => executeById("thread.untrash")}>
                      <RotateCcw size={17} />
                    </ActionButton>
                  </HoverTooltip>
                ) : (
                  <HoverTooltip label="Trash" shortcut="#" placement="bottom">
                    <ActionButton label="Trash" shortcut="#" onClick={() => executeById("thread.trash")}>
                      <Trash2 size={17} />
                    </ActionButton>
                  </HoverTooltip>
                )}
                <HoverTooltip label="Mark spam" shortcut="!" placement="bottom">
                  <ActionButton label="Mark spam" shortcut="!" onClick={() => executeById("thread.spam")}>
                    <ShieldAlert size={17} />
                  </ActionButton>
                </HoverTooltip>
              </div>
            </header>
            {visibleDetail.thread.summary || summaryPending || summaryError ? (
              <div
                className={`thread-summary ${summaryExpanded ? "thread-summary-expanded" : "thread-summary-collapsed"}`}
              >
                {summaryPending ? (
                  <div className="thread-summary-pending">
                    <Sparkles size={14} />
                    <span>Summarizing…</span>
                  </div>
                ) : summaryError ? (
                  <div className="thread-summary-error">
                    <span>{summaryError}</span>
                    <button type="button" onClick={() => void runSummarize()}>
                      Try again
                    </button>
                  </div>
                ) : summaryExpanded && visibleDetail.thread.summary ? (
                  <div className="thread-summary-body">
                    <div className="thread-summary-heading">
                      <Sparkles size={14} />
                      <span>Summary</span>
                    </div>
                    <ul>
                      {summaryLines(visibleDetail.thread.summary).map((line, index) => (
                        <li key={index}>{line}</li>
                      ))}
                    </ul>
                    {visibleDetail.thread.summaryGeneratedAt
                    && visibleDetail.thread.lastMessageAt > visibleDetail.thread.summaryGeneratedAt ? (
                      <p className="thread-summary-stale">New Messages since this summary.</p>
                    ) : null}
                    <div className="thread-summary-actions">
                      <button type="button" onClick={() => setSummaryExpanded(false)}>
                        <ChevronUp size={14} />
                        Collapse Summary
                      </button>
                      <button type="button" onClick={() => void runSummarize()}>
                        <RefreshCw size={13} />
                        Regenerate
                      </button>
                    </div>
                  </div>
                ) : visibleDetail.thread.summary ? (
                  <button
                    type="button"
                    className="thread-summary-pill"
                    onClick={() => setSummaryExpanded(true)}
                  >
                    <Sparkles size={14} />
                    <span className="thread-summary-preview">{summaryPreview(visibleDetail.thread.summary)}</span>
                    <span className="thread-summary-expand">
                      Expand Summary
                      <ChevronDown size={14} />
                    </span>
                  </button>
                ) : null}
              </div>
            ) : null}
            <div className="message-stack" ref={messageStackRef}>
              {displayedMessages.map((message, index) => {
                const isLatest = index === displayedMessages.length - 1;
                const isExpanded = messageExpansionOverrides.get(message.id) ?? (isLatest || message.unread);
                const isActive = (activeMessageId ?? latestDisplayedMessageId) === message.id;
                const parsedSender = parseAddress(message.sender);
                const senderAccount = accounts.find(
                  (account) => account.email.toLocaleLowerCase() === parsedSender.email.toLocaleLowerCase(),
                );
                const senderName = senderAccount?.displayName?.trim() || parsedSender.name;
                const senderDisplayName = formatDisplayName(senderName);
                const recipients = splitAddressList(message.recipients.join(", "));
                const referencedContentIds = referencedImageContentIds(message.bodyHtml);
                const downloadableAttachments = message.attachments.filter(
                  (attachment) => !isInlineImageAttachment(attachment, referencedContentIds),
                );
                const queuedItem = correspondence.outbox.find((item) => `outbox-${item.id}` === message.id);
                const cardBodyId = `message-body-${index}`;
                const activateMessage = () => {
                  activeMessageIdRef.current = message.id;
                  setActiveMessageId(message.id);
                };
                const toggleMessage = () => {
                  activateMessage();
                  pendingMessageToggleFocusRef.current = message.id;
                  setMessageExpansionOverrides((current) => {
                    const next = new Map(current);
                    next.set(message.id, !isExpanded);
                    return next;
                  });
                };
                const registerMessageNode = (node: HTMLElement | null) => {
                  if (isLatest) latestMessageRef.current = node;
                  if (node) messageRefs.current.set(message.id, node);
                  else messageRefs.current.delete(message.id);
                };
                if (!isExpanded) {
                  return (
                    <article
                      className={`message message-card message-card-collapsed ${isActive ? "message-active" : ""}`}
                      key={message.id}
                      ref={registerMessageNode}
                      data-message-id={message.id}
                      onFocusCapture={activateMessage}
                      onMouseDown={activateMessage}
                    >
                      <header className="message-card-header">
                        <button
                          type="button"
                          className="message-card-toggle"
                          aria-expanded={false}
                          aria-controls={cardBodyId}
                          onClick={toggleMessage}
                          onKeyDown={(event) => {
                            if (event.key !== "Enter") return;
                            event.preventDefault();
                            toggleMessage();
                          }}
                        >
                          <span className="message-card-sender">{senderDisplayName}</span>
                          <span className="message-card-snippet">{messageSnippet(message.bodyText)}</span>
                          {downloadableAttachments.length > 0 ? <Paperclip size={13} aria-label="Has attachments" /> : null}
                          <time>{formatMailTimestamp(message.sentAt)}</time>
                          <ChevronDown size={14} className="message-card-chevron" />
                        </button>
                      </header>
                      <div id={cardBodyId} hidden />
                    </article>
                  );
                }
                const replyToMessage = () => {
                  if (selected) {
                    recordTriageEvent({
                      threadId: selected.id,
                      kind: "response",
                      context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
                    });
                  }
                  correspondence.context.reply(message.id);
                };
                const replyAllToMessage = () => {
                  if (selected) {
                    recordTriageEvent({
                      threadId: selected.id,
                      kind: "response",
                      context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
                    });
                  }
                  correspondence.context.replyAll(message.id);
                };
                const forwardMessage = () => {
                  if (selected) {
                    recordTriageEvent({
                      threadId: selected.id,
                      kind: "response",
                      context: mailbox === "inbox" && !includeArchived ? "inbox" : "other",
                    });
                  }
                  correspondence.context.forward(message.id);
                };
                const headerDetails = (
                  <div className="message-header-details">
                    <div className="message-sender-row">
                      <strong><AddressWithCopy address={message.sender} displayName={senderDisplayName} /></strong>
                      {queuedItem ? null : (
                        <div className="message-header-actions">
                          <HoverTooltip label="Reply" placement="bottom">
                            <button
                              type="button"
                              className="message-header-action"
                              aria-label="Reply"
                              onClick={replyToMessage}
                            >
                              <Reply size={14} />
                            </button>
                          </HoverTooltip>
                          <HoverTooltip label="Reply all" placement="bottom">
                            <button
                              type="button"
                              className="message-header-action"
                              aria-label="Reply all"
                              onClick={replyAllToMessage}
                            >
                              <ReplyAll size={14} />
                            </button>
                          </HoverTooltip>
                          <HoverTooltip label="Forward" placement="bottom">
                            <button
                              type="button"
                              className="message-header-action"
                              aria-label="Forward"
                              onClick={forwardMessage}
                            >
                              <Forward size={14} />
                            </button>
                          </HoverTooltip>
                        </div>
                      )}
                      <time>{formatMailTimestamp(message.sentAt)}</time>
                    </div>
                    <div className="message-recipients">
                      to{" "}
                      {recipients.map((recipient, recipientIndex) => (
                        <span key={recipient}>
                          {recipientListSeparator(recipientIndex, recipients.length)}
                          <AddressWithCopy
                            address={recipient}
                            displayName={formatDisplayName(parseAddress(recipient).name)}
                          />
                        </span>
                      ))}
                    </div>
                  </div>
                );
                return (
                  <article
                    className={`message message-card message-card-expanded ${isActive ? "message-active" : ""}`}
                    key={message.id}
                    ref={registerMessageNode}
                    data-message-id={message.id}
                    onFocusCapture={activateMessage}
                    onMouseDown={activateMessage}
                  >
                    <header
                      className="message-expanded-header"
                    >
                      {headerDetails}
                      <button
                        type="button"
                        className="message-expanded-toggle"
                        aria-expanded={true}
                        aria-controls={cardBodyId}
                        aria-label={`Collapse message from ${senderDisplayName}, ${formatMailTimestamp(message.sentAt)}`}
                        onClick={toggleMessage}
                        onKeyDown={(event) => {
                          if (event.key !== "Enter") return;
                          event.preventDefault();
                          toggleMessage();
                        }}
                      >
                        <ChevronUp size={14} />
                      </button>
                    </header>
                    <div id={cardBodyId} className="message-card-body">
                      <SafeMessage
                        html={message.bodyHtml}
                        text={message.bodyText}
                        loadImages={loadRemoteImages}
                        imageCacheKey={message.id}
                        onImageClick={setLightboxImageSrc}
                        onEnterKey={toggleMessage}
                        resolveImage={(url) => {
                          if (!/^cid:/i.test(url)) return mailClient.fetchRemoteImage(url);
                          const contentId = normalizeContentId(url.slice(4));
                          const embedded = message.attachments.find((attachment) =>
                            attachment.contentId
                            && normalizeContentId(attachment.contentId) === contentId
                          );
                          if (!embedded) return Promise.reject(new Error("Embedded image not found"));
                          return queuedItem
                            ? mailClient.readInlineImage(queuedItem.draft.id, embedded.id)
                            : mailClient.fetchAttachmentImage(message.id, embedded.id);
                        }}
                        theme={effectiveThemeValue}
                        fontScale={fontScale / 100}
                        fontFamily={fontFamily}
                        tone={isLatest ? "current" : message.unread ? "default" : "muted"}
                      />
                      {downloadableAttachments.length > 0 ? (
                        <div className="message-attachments" aria-label="Attachments">
                          {downloadableAttachments.some(isCalendarAttachment) ? (
                            <CalendarAttachmentGroup
                              messageId={message.id}
                              attachments={downloadableAttachments.filter(isCalendarAttachment)}
                              onError={(notice) => setNotice({ message: notice })}
                            />
                          ) : null}
                          {downloadableAttachments.filter((attachment) => !isCalendarAttachment(attachment)).map((attachment) => (
                              <div className="message-attachment" key={attachment.id}>
                                <button
                                  type="button"
                                  className="attachment-badge"
                                  aria-label={`View ${attachment.filename}`}
                                  onClick={() => {
                                    void mailClient.openAttachment(message.id, attachment.id).catch((reason: unknown) => {
                                      setNotice({ message: `Could not open attachment: ${reason instanceof Error ? reason.message : String(reason)}` });
                                    });
                                  }}
                                >
                                  <Paperclip size={14} />
                                  <span>{attachment.filename}</span>
                                  <small>{formatAttachmentSize(attachment.size)}</small>
                                  <ExternalLink size={13} />
                                </button>
                                <button
                                  type="button"
                                  className="attachment-download"
                                  aria-label={`Download ${attachment.filename}`}
                                  title={`Download ${attachment.filename}`}
                                  onClick={() => {
                                    void mailClient.saveAttachment(message.id, attachment.id).catch((reason: unknown) => {
                                      setNotice({ message: `Could not download attachment: ${reason instanceof Error ? reason.message : String(reason)}` });
                                    });
                                  }}
                                >
                                  <Download size={14} />
                                </button>
                              </div>
                          ))}
                        </div>
                      ) : null}
                    </div>
                  </article>
                );
              })}
              {composerBelongsToVisibleThread ? correspondence.composer : null}
            </div>
          </>
        ) : detailLoading ? (
          <div className="reader-empty" role="status">
            <p>Loading conversation…</p>
          </div>
        ) : (
          <div className="reader-empty">
            <Mail size={28} />
            <p>
              {mailbox === "drafts"
                ? "Select a draft to open it for editing"
                : mailbox === "outbox"
                  ? "Delivery details are shown in the list"
                  : "Select a conversation"}
            </p>
          </div>
        )}
      </section>

      {calendarOpen ? <CalendarSidebar onClose={() => setCalendarOpen(false)} /> : null}

      {correspondence.overlay}
      {unsubscribeMessage ? (
        <UnsubscribeConfirm
          message={unsubscribeMessage}
          onClose={() => setUnsubscribeMessageId(null)}
          onConfirm={confirmUnsubscribe}
        />
      ) : null}
      {paletteOpen ? (
        <CommandPalette
          context={context}
          execute={executeCommand}
          extraCommands={paletteExtraCommands}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {shortcutHelpOpen ? (
        <ShortcutHelp extraCommands={paletteExtraCommands} onClose={() => setShortcutHelpOpen(false)} />
      ) : null}
      {diagnosticsOpen ? (
        <Diagnostics
          status={syncStatus}
          recovery={recoveryStatus}
          onClose={() => setDiagnosticsOpen(false)}
        />
      ) : null}
      {labelTargetIds && labelTargetIds.length > 0 ? (
        <LabelManager
          labels={labels}
          checkedLabelIds={new Set(
            labels
              .filter((label) => label.kind === "user")
              .filter((label) =>
                labelTargetIds.every((id) => threads.find((thread) => thread.id === id)?.labels.includes(label.id)),
              )
              .map((label) => label.id),
          )}
          onClose={() => setLabelTargetIds(null)}
          onCreate={async (name) => {
            const label = await mailClient.createLabel(name);
            setLabels((current) => [...current, label]);
          }}
          onDelete={async (id) => {
            await mailClient.deleteLabel(id);
            setLabels((current) => current.filter((label) => label.id !== id));
            await loadThreads(query);
          }}
          onRename={async (id, name) => {
            const updated = await mailClient.updateLabel(id, name);
            setLabels((current) =>
              current.map((label) => label.id === id ? { ...label, ...updated } : label),
            );
          }}
          onToggle={(labelId, value) => {
            const label = labels.find((candidate) => candidate.id === labelId);
            executeCommand(labelCommand(labelId, label?.name ?? "Label", value));
          }}
        />
      ) : null}
      {settingsOpen ? (
        <Settings
          section={settingsSection}
          onSectionChange={setSettingsSection}
          onClose={() => setSettingsOpen(false)}
          theme={theme}
          onThemeChange={setTheme}
          fontScale={fontScale}
          onFontScaleChange={setFontScale}
          fontFamily={fontFamily}
          onFontFamilyChange={setFontFamily}
          autoReadDelaySeconds={autoReadDelaySeconds}
          onAutoReadDelayChange={setAutoReadDelaySeconds}
          loadRemoteImages={loadRemoteImages}
          onLoadRemoteImagesChange={setLoadRemoteImages}
          onAiConfigChange={refreshAiAvailability}
          authStatus={authStatus}
          accounts={accounts}
          calendarAccounts={calendarAccounts}
          activeAccountId={activeAccountId}
          onAddAccount={async () => {
            await mailClient.addAccount();
            setAuthStatus(await mailClient.googleAuthStatus());
            await refreshAccounts();
          }}
          onRemoveAccount={async (email) => {
            await mailClient.removeAccount(email);
            const wasActive = activeAccountId === email;
            if (wasActive) setActiveAccountId(null);
            await refreshAccounts();
            void loadThreads(query, wasActive ? null : undefined);
            setAuthStatus(await mailClient.googleAuthStatus());
          }}
          onReconnectAccount={async (email) => {
            await mailClient.reconnectAccount(email);
            await refreshAccounts();
            setAuthStatus(await mailClient.googleAuthStatus());
          }}
          onSetAccountDisplayName={async (email, displayName) => {
            await mailClient.setAccountDisplayName(email, displayName);
            await refreshAccounts();
          }}
          onSetAccountColor={async (email, color) => {
            await mailClient.setAccountColor(email, color);
            await refreshAccounts();
          }}
          onReorderAccounts={reorderAccounts}
          onAddCalendarAccount={async () => {
            await mailClient.addCalendarAccount();
            await refreshCalendarAccounts();
          }}
          onReconnectCalendarAccount={async (email) => {
            await mailClient.reconnectCalendarAccount(email);
            await refreshCalendarAccounts();
          }}
          onRemoveCalendarAccount={async (email) => {
            await mailClient.removeCalendarAccount(email);
            const remaining = await refreshCalendarAccounts();
            if (remaining.length === 0) setCalendarOpen(false);
          }}
          onSettingsImported={async (result) => {
            const { preferences } = result;
            setTheme(preferences.theme);
            setFontScale(preferences.fontScale);
            setFontFamily(preferences.fontFamily);
            setAutoReadDelaySeconds(preferences.autoReadDelaySeconds);
            setLoadRemoteImages(preferences.loadRemoteImages);
            setActiveAccountId(preferences.selectedAccountId);
            await Promise.all([
              refreshAccounts(),
              refreshSplitInboxes(),
              mailClient.googleAuthStatus().then(setAuthStatus),
            ]);
            refreshAiAvailability();
            setSettingsSection("accounts");
          }}
          splitInboxes={splitInboxes}
          labelsByAccount={labelsByAccount}
          onCreateSplitInbox={async (name, matchKind, matchValue, accountId) => {
            const created = await mailClient.createSplitInbox(name, matchKind, matchValue, accountId);
            setSplitInboxes((current) => [...current, created]);
          }}
          onRenameSplitInbox={async (id, name) => {
            const updated = await mailClient.updateSplitInbox(id, name);
            setSplitInboxes((current) =>
              current.map((splitInbox) => splitInbox.id === id ? { ...splitInbox, ...updated } : splitInbox),
            );
          }}
          onDeleteSplitInbox={async (id) => {
            await mailClient.deleteSplitInbox(id);
            setSplitInboxes((current) => current.filter((splitInbox) => splitInbox.id !== id));
          }}
          onReorderSplitInboxes={async (ids) => {
            await mailClient.reorderSplitInboxes(ids);
            await refreshSplitInboxes();
          }}
        />
      ) : null}
      {notice ? (
        <div className="toast" role="status">
          {notice.message}
          {notice.undo ? <button onClick={notice.undo}>Undo</button> : null}
          <button aria-label="Dismiss" onClick={() => setNotice(null)}><X size={14} /></button>
        </div>
      ) : null}
      {lightboxImageSrc ? (
        <ImageLightbox src={lightboxImageSrc} onClose={() => setLightboxImageSrc(null)} />
      ) : null}
    </main>
  );
}

function AddressWithCopy({ address, displayName }: { address: string; displayName?: string }) {
  const [copied, setCopied] = useState(false);
  const parsedAddress = parseAddress(address);
  const parsed = displayName ? { ...parsedAddress, name: displayName } : parsedAddress;

  const handleCopy = async (event: React.MouseEvent) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(parsed.email);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <span className="address" tabIndex={0} onClick={(event) => event.stopPropagation()}>
      <span className="address-name">{parsed.name}</span>
      <span className="address-popover">
        <span className="address-email">{parsed.email}</span>
        <button
          type="button"
          className="address-copy"
          aria-label={copied ? "Copied" : `Copy ${parsed.email}`}
          onClick={handleCopy}
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
        </button>
      </span>
    </span>
  );
}

function messageSnippet(bodyText: string, maxLength = 140): string {
  const collapsed = decodeHtmlEntities(bodyText).replace(/\s+/g, " ").trim();
  return collapsed.length > maxLength ? `${collapsed.slice(0, maxLength).trimEnd()}…` : collapsed;
}

function splitDraftRecipients(draft: Draft): string[] {
  const result: string[] = [];
  for (const field of [draft.to, draft.cc]) {
    let start = 0;
    let quoted = false;
    let angleDepth = 0;
    for (let index = 0; index <= field.length; index++) {
      const character = field[index];
      if (character === '"' && field[index - 1] !== "\\") quoted = !quoted;
      else if (!quoted && character === "<") angleDepth++;
      else if (!quoted && character === ">") angleDepth = Math.max(0, angleDepth - 1);
      if (index === field.length || (character === "," && !quoted && angleDepth === 0)) {
        const address = field.slice(start, index).trim();
        if (address) result.push(address);
        start = index + 1;
      }
    }
  }
  return result;
}

function comparableMessageBody(value: string): string {
  return decodeHtmlEntities(value).replace(/\s+/g, " ").trim().toLocaleLowerCase();
}

function providerMessageMatchesDraft(message: Message, draft: Draft): boolean {
  if (parseAddress(message.sender).email.toLocaleLowerCase() !== draft.account.toLocaleLowerCase()) return false;
  if (new Date(message.sentAt).getTime() < draft.updatedAt - 60_000) return false;
  const actual = comparableMessageBody(message.bodyText);
  const expected = comparableMessageBody(draft.body);
  return Boolean(expected) && (actual === expected || actual.includes(expected) || expected.includes(actual));
}

/**
 * Adds locally queued replies to their open conversation immediately. Once
 * Gmail's copy reaches the thread cache it wins, preventing a duplicate.
 */
export function messagesWithQueuedReplies(detail: ThreadDetail, outbox: OutboxItem[]): Message[] {
  const queuedReplies = outbox
    .filter((item) =>
      ["reply", "replyAll"].includes(item.draft.mode)
      && !["canceled", "failed"].includes(item.state)
      && Boolean(item.draft.sourceId)
      && detail.messages.some((message) => message.id === item.draft.sourceId)
      && !detail.messages.some((message) =>
        (item.providerId && message.id === item.providerId)
        || providerMessageMatchesDraft(message, item.draft))
      )
    .sort((left, right) => left.draft.updatedAt - right.draft.updatedAt)
    .map<Message>((item) => ({
      id: `outbox-${item.id}`,
      threadId: detail.thread.id,
      sender: item.draft.account,
      recipients: splitDraftRecipients(item.draft),
      sentAt: new Date(item.draft.updatedAt).toISOString(),
      bodyHtml: item.draft.bodyHtml ?? "",
      bodyText: item.draft.body,
      unread: false,
      unsubscribe: null,
      attachments: item.draft.attachments.map((attachment) => ({
        id: attachment.id,
        filename: attachment.name,
        mimeType: attachment.mime,
        size: attachment.size,
        contentId: attachment.contentId,
        inline: attachment.inline,
      })),
    }));

  return queuedReplies.length > 0 ? [...detail.messages, ...queuedReplies] : detail.messages;
}

function recipientListSeparator(index: number, recipientCount: number): string {
  if (index === 0) return "";
  if (index === recipientCount - 1) return recipientCount === 2 ? " and " : ", and ";
  return ", ";
}

function summaryLines(summary: string): string[] {
  return summary
    .split("\n")
    .map((line) => line.replace(/^[-•]\s*/, "").trim())
    .filter(Boolean);
}

function summaryPreview(summary: string, maxLength = 90): string {
  const [first] = summaryLines(summary);
  if (!first) return "";
  return first.length > maxLength ? `${first.slice(0, maxLength).trimEnd()}…` : first;
}

export function AccountSwitcher({
  accounts,
  unreadCounts,
  activeAccountId,
  onSwitch,
  onShowAll,
  onReorder,
}: {
  accounts: Account[];
  unreadCounts: UnreadCounts;
  activeAccountId: string | null;
  onSwitch(email: string): void;
  onShowAll(): void;
  onReorder(emails: string[]): void;
}) {
  const [draggedEmail, setDraggedEmail] = useState<string | null>(null);
  const [dragOverEmail, setDragOverEmail] = useState<string | null>(null);
  const draggedEmailRef = useRef<string | null>(null);

  if (accounts.length <= 1) return null;

  const totalUnread = accounts.reduce((total, account) => total + (unreadCounts[account.email] ?? 0), 0);

  const clearDragState = () => {
    draggedEmailRef.current = null;
    setDraggedEmail(null);
    setDragOverEmail(null);
  };

  const handleDrop = (targetEmail: string, transferredEmail: string) => {
    const sourceEmail = draggedEmailRef.current ?? transferredEmail;
    if (sourceEmail && sourceEmail !== targetEmail) {
      const from = accounts.findIndex((account) => account.email === sourceEmail);
      const to = accounts.findIndex((account) => account.email === targetEmail);
      if (from !== -1 && to !== -1) {
        const next = [...accounts];
        const [moved] = next.splice(from, 1);
        next.splice(to, 0, moved!);
        onReorder(next.map((account) => account.email));
      }
    }
    clearDragState();
  };

  return (
    <div className="account-rail" role="radiogroup" aria-label="Filter by account">
      <HoverTooltip label="All accounts">
        <button
          type="button"
          role="radio"
          aria-checked={activeAccountId === null}
          aria-label={totalUnread > 0 ? `All accounts, ${totalUnread} unread` : "All accounts"}
          className={`account-icon all-accounts ${activeAccountId === null ? "active" : ""}`}
          onClick={onShowAll}
        >
          {totalUnread > 0 ? <UnreadBadge count={totalUnread} /> : null}
        </button>
      </HoverTooltip>
      {accounts.map((account) => {
        const name = account.displayName ?? account.email;
        const unreadCount = unreadCounts[account.email] ?? 0;
        return (
          <HoverTooltip key={account.email} label={name}>
            <button
              type="button"
              role="radio"
              aria-checked={activeAccountId === account.email}
              aria-label={unreadCount > 0 ? `${name}, ${unreadCount} unread` : name}
              title="Drag to reorder accounts"
              draggable
              className={`account-icon ${activeAccountId === account.email ? "active" : ""} ${draggedEmail === account.email ? "dragging" : ""} ${dragOverEmail === account.email && draggedEmail !== account.email ? "drag-over" : ""}`}
              style={{ background: account.color }}
              onClick={() => onSwitch(account.email)}
              onDragStart={(event) => {
                draggedEmailRef.current = account.email;
                setDraggedEmail(account.email);
                event.dataTransfer.effectAllowed = "move";
                event.dataTransfer.setData("text/plain", account.email);
              }}
              onDragEnter={(event) => {
                event.preventDefault();
                if (draggedEmail && draggedEmail !== account.email) setDragOverEmail(account.email);
              }}
              onDragOver={(event) => {
                event.preventDefault();
                event.dataTransfer.dropEffect = "move";
              }}
              onDragLeave={() => setDragOverEmail((current) => (current === account.email ? null : current))}
              onDrop={(event) => {
                event.preventDefault();
                handleDrop(account.email, event.dataTransfer.getData("text/plain"));
              }}
              onDragEnd={clearDragState}
            >
              {name.charAt(0).toUpperCase()}
              {unreadCount > 0 ? <UnreadBadge count={unreadCount} /> : null}
            </button>
          </HoverTooltip>
        );
      })}
    </div>
  );
}

function UnreadBadge({ count }: { count: number }) {
  return <span className="account-unread-badge" aria-hidden="true">{count > 99 ? "99+" : count}</span>;
}

function FiltersButton({
  activeFilters,
  onToggleFilter,
}: {
  activeFilters: Set<MessageFilterKind>;
  onToggleFilter(kind: MessageFilterKind): void;
}) {
  const [open, setOpen] = useState(false);
  const anchorRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (anchorRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  return (
    <div className="filters-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filters-trigger ${activeFilters.size > 0 ? "active" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <ListFilter size={15} />
        <span>Filters</span>
        {activeFilters.size > 0 ? <span className="filters-badge">{activeFilters.size}</span> : null}
      </button>
      {open ? (
        <FiltersMenu
          activeFilters={activeFilters}
          onToggleFilter={onToggleFilter}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </div>
  );
}

function FiltersMenu({
  activeFilters,
  onToggleFilter,
  onClose,
}: {
  activeFilters: Set<MessageFilterKind>;
  onToggleFilter(kind: MessageFilterKind): void;
  onClose(): void;
}) {
  useEscapeDismiss(onClose);
  return (
    <div className="filters-menu" role="menu" aria-label="Filters">
      <div className="filters-menu-title">Filters</div>
      {MESSAGE_FILTER_OPTIONS.map((option) => {
        const isActive = activeFilters.has(option.kind);
        return (
          <button
            key={option.kind}
            type="button"
            role="menuitemcheckbox"
            aria-checked={isActive}
            className={`filters-menu-item ${isActive ? "active" : ""}`}
            onClick={() => onToggleFilter(option.kind)}
          >
            <span className="filters-menu-item-label">
              <span className="filters-menu-item-check" aria-hidden="true">
                {isActive ? <Check size={13} /> : null}
              </span>
              {option.label}
            </span>
            <span className="filters-menu-item-keys">
              <kbd>shift</kbd>
              <kbd>{option.shortcutKey}</kbd>
            </span>
          </button>
        );
      })}
    </div>
  );
}

function HoverTooltip({
  children,
  label,
  placement = "right",
  shortcut,
}: {
  children: React.ReactNode;
  label: string;
  placement?: "right" | "bottom";
  shortcut?: string;
}) {
  return (
    <span className={`tooltip-anchor tooltip-${placement}`}>
      {children}
      <span className="hover-tooltip" role="tooltip">
        <strong>{label}</strong>
        {shortcut ? <kbd>{shortcut}</kbd> : null}
      </span>
    </span>
  );
}

function ActionButton({
  children,
  label,
  onClick,
  shortcut,
}: {
  children: React.ReactNode;
  label: string;
  onClick(): void;
  shortcut?: string;
}) {
  return (
    <button className="action-button" aria-label={shortcut ? `${label} (${shortcut})` : label} onClick={onClick}>
      {children}<span>{label}</span>{shortcut ? <kbd>{shortcut}</kbd> : null}
    </button>
  );
}

function CommandPalette({
  context,
  execute,
  extraCommands = [],
  onClose,
}: {
  context: CommandContext;
  execute(command: Command): void;
  extraCommands?: Command[];
  onClose(): void;
}) {
  const [filter, setFilter] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => inputRef.current?.focus(), []);
  const visible = [...commands, ...extraCommands].filter((command) =>
    command.title.toLocaleLowerCase().includes(filter.toLocaleLowerCase()),
  );
  return (
    <Modal title="Command palette" onClose={onClose}>
      <label className="palette-search">
        <Search size={18} />
        <input
          ref={inputRef}
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Type a command"
          aria-label="Filter commands"
        />
      </label>
      <div className="command-list">
        {visible.map((command) => (
          <button
            key={command.id}
            disabled={!command.enabled(context)}
            onClick={() => {
              execute(command);
              onClose();
            }}
          >
            <span><small>{command.group}</small>{command.title}</span>
            <span>{command.keys.map((key) => <kbd key={key}>{key}</kbd>)}</span>
          </button>
        ))}
      </div>
    </Modal>
  );
}

const shortcutGroupOrder = ["Navigation", "Triage", "Compose", "Application"] as const;

function ShortcutHelp({
  extraCommands = [],
  onClose,
}: {
  extraCommands?: Command[];
  onClose(): void;
}) {
  const shortcutCommands = [
    ...[...commands, ...extraCommands].filter((command) => command.keys.length > 0),
    ...formattingShortcuts.map((shortcut) => ({
      id: shortcut.id,
      title: shortcut.title,
      keys: [shortcut.key],
      group: "Compose" as const,
    })),
  ];
  return (
    <Modal title="Keyboard shortcuts" className="shortcut-help-modal" onClose={onClose}>
      <p className="shortcut-help-intro">Use ThreeStrands without leaving the keyboard.</p>
      <div className="shortcut-help-groups">
        {shortcutGroupOrder.map((group) => {
          const groupCommands = shortcutCommands.filter((command) => command.group === group);
          if (groupCommands.length === 0) return null;
          return (
            <section key={group} aria-labelledby={`shortcut-group-${group.toLowerCase()}`}>
              <h3 id={`shortcut-group-${group.toLowerCase()}`}>{group}</h3>
              <dl>
                {groupCommands.map((command) => (
                  <div key={command.id}>
                    <dt>{command.title}</dt>
                    <dd>
                      {command.keys.map((key) => <ShortcutKeys key={key} shortcut={key} />)}
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          );
        })}
      </div>
    </Modal>
  );
}

function ShortcutKeys({ shortcut }: { shortcut: string }) {
  const steps = shortcutSteps(shortcut);
  return (
    <span className="shortcut-keys">
      {steps.map((step, index) => (
        <span key={step}>
          {index > 0 ? <small>then</small> : null}
          <kbd>{step.replace("Mod", "⌘/Ctrl").replaceAll("+", " + ")}</kbd>
        </span>
      ))}
    </span>
  );
}

function recoveryStatusMessage(recovery: RecoveryStatus): string {
  switch (recovery.kind) {
    case "restoredFromBackup":
      return "Your mail cache was damaged and has been restored from its most recent local backup. " +
        "A few of the most recent changes may be missing until the next sync.";
    case "freshDatabase":
      return "Your mail cache was damaged and could not be restored from a backup, so it was rebuilt " +
        "from scratch. Your mail is safe on the server; ThreeStrands is resyncing it now.";
  }
}

export function Diagnostics({
  status,
  recovery,
  onClose,
}: {
  status: SyncStatus | null;
  recovery?: RecoveryStatus | null;
  onClose(): void;
}) {
  return (
    <Modal title="Sync diagnostics" onClose={onClose}>
      <dl className="diagnostics">
        {recovery ? (
          <>
            <dt>Database recovery</dt>
            <dd className="recovery-notice">{recoveryStatusMessage(recovery)}</dd>
          </>
        ) : null}
        <dt>State</dt><dd>{status?.state ?? "unknown"}</dd>
        <dt>Last successful sync</dt>
        <dd>{status?.lastSuccessfulSync ? new Date(status.lastSuccessfulSync).toLocaleString() : "Never"}</dd>
        <dt>History cursor</dt><dd>{status?.cursor ?? "Not initialized"}</dd>
        <dt>Pending mutations</dt><dd>{status?.pendingMutations ?? 0}</dd>
        <dt>Permanently failed operations</dt>
        <dd>
          {status?.failedMutations?.length ? (
            <ul className="failed-mutations">
              {status.failedMutations.map((mutation) => (
                <li key={mutation.id}>
                  <strong>{mutation.kind}</strong>
                  {" · "}
                  {mutation.error}
                  <small>
                    {mutation.attempts} {mutation.attempts === 1 ? "attempt" : "attempts"}
                    {" · "}
                    {new Date(mutation.createdAt).toLocaleString()}
                  </small>
                </li>
              ))}
            </ul>
          ) : "None"}
        </dd>
        <dt>Quarantined messages</dt>
        <dd>
          {status?.quarantinedMessages?.length ? (
            <ul className="failed-mutations">
              {status.quarantinedMessages.map((message) => (
                <li key={`${message.threadId}:${message.messageId}`}>
                  <strong>Message {message.messageId}</strong>
                  {" · "}
                  {message.error}
                  <small>
                    Thread {message.threadId}
                    {" · "}
                    {new Date(message.createdAt).toLocaleString()}
                  </small>
                </li>
              ))}
            </ul>
          ) : "None"}
        </dd>
        <dt>Last error</dt><dd>{status?.error ?? "None"}</dd>
      </dl>
    </Modal>
  );
}

function LabelManager({
  labels,
  checkedLabelIds,
  onClose,
  onCreate,
  onDelete,
  onRename,
  onToggle,
}: {
  labels: Label[];
  checkedLabelIds: Set<string>;
  onClose(): void;
  onCreate(name: string): Promise<void>;
  onDelete(id: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onToggle(id: string, value: boolean): void;
}) {
  const [name, setName] = useState("");
  const [renaming, setRenaming] = useState<Label | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [busy, setBusy] = useState(false);
  const userLabels = labels
    .filter((label) => label.kind === "user")
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
  const labelInputRefs = useRef<Record<string, HTMLInputElement | null>>({});

  useEffect(() => {
    const firstLabel = userLabels[0];
    if (!firstLabel) return;
    labelInputRefs.current[firstLabel.id]?.focus();
  }, [userLabels.length]);

  const moveLabelFocus = (labelId: string, direction: -1 | 1) => {
    const currentIndex = userLabels.findIndex((label) => label.id === labelId);
    if (currentIndex === -1) return;
    const nextIndex = Math.max(0, Math.min(userLabels.length - 1, currentIndex + direction));
    if (nextIndex === currentIndex) return;
    labelInputRefs.current[userLabels[nextIndex]?.id ?? ""]?.focus();
  };

  return (
    <Modal title="Manage labels" onClose={onClose}>
      <form
        className="create-label"
        onSubmit={(event) => {
          event.preventDefault();
          if (!name.trim() || busy) return;
          setBusy(true);
          void onCreate(name).then(() => setName("")).finally(() => setBusy(false));
        }}
      >
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="New label name"
          aria-label="New label name"
        />
        <button type="submit" disabled={!name.trim() || busy}>Create</button>
      </form>
      <div className="label-list">
        {userLabels.map((label) => (
          <div key={label.id}>
            <label>
              <input
                ref={(input) => {
                  labelInputRefs.current[label.id] = input;
                }}
                type="checkbox"
                checked={checkedLabelIds.has(label.id)}
                onChange={(event) => onToggle(label.id, event.target.checked)}
                onKeyDown={(event) => {
                  if (event.key === "ArrowUp") {
                    event.preventDefault();
                    moveLabelFocus(label.id, -1);
                  } else if (event.key === "ArrowDown") {
                    event.preventDefault();
                    moveLabelFocus(label.id, 1);
                  } else if (event.key === " " || event.key === "Spacebar" || event.key === "Space" || event.code === "Space") {
                    event.preventDefault();
                    onToggle(label.id, !checkedLabelIds.has(label.id));
                  }
                }}
              />
              <span className="label-color" style={{ background: label.color ?? "#64646d" }} />
              {label.name}
            </label>
            <span className="label-actions">
              <button
                aria-label={`Rename ${label.name}`}
                onClick={() => {
                  setRenaming(label);
                  setRenameValue(label.name);
                }}
              >
                <Pencil size={14} />
              </button>
              <button aria-label={`Delete ${label.name}`} onClick={() => void onDelete(label.id)}>
                <Trash2 size={14} />
              </button>
            </span>
          </div>
        ))}
        {labels.every((label) => label.kind !== "user") ? (
          <p className="empty">No labels yet. Create one below.</p>
        ) : null}
      </div>
      {renaming ? (
        <form
          className="rename-label"
          onSubmit={(event) => {
            event.preventDefault();
            if (!renameValue.trim() || busy) return;
            setBusy(true);
            void onRename(renaming.id, renameValue)
              .then(() => setRenaming(null))
              .finally(() => setBusy(false));
          }}
        >
          <input
            autoFocus
            value={renameValue}
            onChange={(event) => setRenameValue(event.target.value)}
            aria-label={`Rename ${renaming.name}`}
          />
          <button type="submit" disabled={!renameValue.trim() || busy}>Save</button>
          <button type="button" onClick={() => setRenaming(null)}>Cancel</button>
        </form>
      ) : null}
    </Modal>
  );
}

const SETTINGS_SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: "appearance", label: "Appearance" },
  { id: "reading", label: "Reading" },
  { id: "accounts", label: "Mail Accounts" },
  { id: "calendarAccounts", label: "Calendar Accounts" },
  { id: "splitInboxes", label: "Split Inboxes" },
  { id: "ai", label: "AI provider" },
  { id: "privacy", label: "Privacy" },
  { id: "data", label: "Data transfer" },
];

function Settings({
  section,
  onSectionChange,
  onClose,
  theme,
  onThemeChange,
  fontScale,
  onFontScaleChange,
  fontFamily,
  onFontFamilyChange,
  autoReadDelaySeconds,
  onAutoReadDelayChange,
  loadRemoteImages,
  onLoadRemoteImagesChange,
  onAiConfigChange,
  authStatus,
  accounts,
  calendarAccounts,
  activeAccountId,
  onAddAccount,
  onRemoveAccount,
  onReconnectAccount,
  onSetAccountDisplayName,
  onSetAccountColor,
  onReorderAccounts,
  onAddCalendarAccount,
  onReconnectCalendarAccount,
  onRemoveCalendarAccount,
  onSettingsImported,
  splitInboxes,
  labelsByAccount,
  onCreateSplitInbox,
  onRenameSplitInbox,
  onDeleteSplitInbox,
  onReorderSplitInboxes,
}: {
  section: SettingsSection;
  onSectionChange(section: SettingsSection): void;
  onClose(): void;
  theme: Theme;
  onThemeChange(theme: Theme): void;
  fontScale: number;
  onFontScaleChange(value: number): void;
  fontFamily: FontFamily;
  onFontFamilyChange(value: FontFamily): void;
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
  loadRemoteImages: boolean;
  onLoadRemoteImagesChange(value: boolean): void;
  onAiConfigChange(): void;
  authStatus: AuthStatus | null;
  accounts: Account[];
  calendarAccounts: CalendarAccount[];
  activeAccountId: string | null;
  onAddAccount(): Promise<void>;
  onRemoveAccount(email: string): Promise<void>;
  onReconnectAccount(email: string): Promise<void>;
  onSetAccountDisplayName(email: string, displayName: string | null): Promise<void>;
  onSetAccountColor(email: string, color: string): Promise<void>;
  onReorderAccounts(emails: string[]): Promise<void>;
  onAddCalendarAccount(): Promise<void>;
  onReconnectCalendarAccount(email: string): Promise<void>;
  onRemoveCalendarAccount(email: string): Promise<void>;
  onSettingsImported(result: SettingsImportResult): Promise<void>;
  splitInboxes: SplitInbox[];
  labelsByAccount: Record<string, Label[]>;
  onCreateSplitInbox(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onRenameSplitInbox(id: string, name: string): Promise<void>;
  onDeleteSplitInbox(id: string): Promise<void>;
  onReorderSplitInboxes(ids: string[]): Promise<void>;
}) {
  return (
    <Modal title="Settings" className="settings-modal" onClose={onClose}>
      <div className="settings-body">
        <nav className="settings-nav" aria-label="Settings sections">
          {SETTINGS_SECTIONS.map((item) => (
            <button
              key={item.id}
              className={item.id === section ? "active" : ""}
              aria-current={item.id === section}
              onClick={() => onSectionChange(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>
        <div className="settings-panel">
          {section === "appearance" ? (
            <AppearanceSettings
              theme={theme}
              onThemeChange={onThemeChange}
              fontScale={fontScale}
              onFontScaleChange={onFontScaleChange}
              fontFamily={fontFamily}
              onFontFamilyChange={onFontFamilyChange}
            />
          ) : null}
          {section === "reading" ? (
            <ReadingSettings
              autoReadDelaySeconds={autoReadDelaySeconds}
              onAutoReadDelayChange={onAutoReadDelayChange}
            />
          ) : null}
          {section === "accounts" ? (
            <AccountsSettings
              authStatus={authStatus}
              accounts={accounts}
              onAdd={onAddAccount}
              onRemove={onRemoveAccount}
              onReconnect={onReconnectAccount}
              onSetDisplayName={onSetAccountDisplayName}
              onSetColor={onSetAccountColor}
              onReorder={onReorderAccounts}
            />
          ) : null}
          {section === "calendarAccounts" ? (
            <CalendarAccountsSettings
              authStatus={authStatus}
              accounts={calendarAccounts}
              onAdd={onAddCalendarAccount}
              onReconnect={onReconnectCalendarAccount}
              onRemove={onRemoveCalendarAccount}
            />
          ) : null}
          {section === "splitInboxes" ? (
            <SplitInboxesSettings
              splitInboxes={splitInboxes}
              accounts={accounts}
              activeAccountId={activeAccountId}
              labelsByAccount={labelsByAccount}
              onCreate={onCreateSplitInbox}
              onRename={onRenameSplitInbox}
              onDelete={onDeleteSplitInbox}
              onReorder={onReorderSplitInboxes}
            />
          ) : null}
          {section === "ai" ? <AiProviderSettings onChange={onAiConfigChange} /> : null}
          {section === "privacy" ? (
            <PrivacySettings
              loadRemoteImages={loadRemoteImages}
              onLoadRemoteImagesChange={onLoadRemoteImagesChange}
            />
          ) : null}
          {section === "data" ? <DataTransferSettings onImported={onSettingsImported} /> : null}
        </div>
      </div>
    </Modal>
  );
}

function AppearanceSettings({
  theme,
  onThemeChange,
  fontScale,
  onFontScaleChange,
  fontFamily,
  onFontFamilyChange,
}: {
  theme: Theme;
  onThemeChange(theme: Theme): void;
  fontScale: number;
  onFontScaleChange(value: number): void;
  fontFamily: FontFamily;
  onFontFamilyChange(value: FontFamily): void;
}) {
  const [fontFamilies, setFontFamilies] = useState<string[]>([]);
  const [fontQuery, setFontQuery] = useState("");
  const [fontsLoading, setFontsLoading] = useState(true);
  const [fontLoadFailed, setFontLoadFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setFontsLoading(true);
    setFontLoadFailed(false);
    listSystemFontFamilies()
      .then((families) => {
        if (!cancelled) setFontFamilies(families);
      })
      .catch(() => {
        if (!cancelled) setFontLoadFailed(true);
      })
      .finally(() => {
        if (!cancelled) setFontsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);
  const normalizedFontQuery = fontQuery.trim().toLocaleLowerCase();
  const visibleFontFamilies = fontFamilies.filter((family) =>
    family.toLocaleLowerCase().includes(normalizedFontQuery)
  );
  const showSystemFont = !normalizedFontQuery
    || "system default".includes(normalizedFontQuery);
  const selectedFontIsInstalled = fontFamily === DEFAULT_FONT_FAMILY
    || fontFamilies.includes(fontFamily);
  const themeOptions: { value: Theme; label: string }[] = [
    { value: "system", label: "Match system" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];
  return (
    <section className="settings-section" aria-label="Appearance">
      <h3>Theme</h3>
      <div className="settings-radio-row" role="radiogroup" aria-label="Theme">
        {themeOptions.map((option) => (
          <label key={option.value}>
            <input
              type="radio"
              name="theme"
              checked={theme === option.value}
              onChange={() => onThemeChange(option.value)}
            />
            {option.label}
          </label>
        ))}
      </div>

      <h3>Font size</h3>
      <div className="settings-row">
        <input
          type="range"
          min={MIN_FONT_SCALE}
          max={MAX_FONT_SCALE}
          step={FONT_SCALE_STEP}
          value={fontScale}
          aria-label="Font size"
          onChange={(event) => onFontScaleChange(Number(event.target.value))}
        />
        <span>{fontScale}%</span>
      </div>

      <h3>Default font</h3>
      <p className="settings-hint">Used throughout the app and for unformatted message text.</p>
      <label className="font-search">
        <Search size={15} aria-hidden="true" />
        <input
          type="search"
          value={fontQuery}
          placeholder="Search installed fonts"
          aria-label="Search installed fonts"
          onChange={(event) => setFontQuery(event.target.value)}
        />
      </label>
      <div className="font-picker" role="radiogroup" aria-label="Default font">
        {showSystemFont ? (
          <label
            className={`font-option${fontFamily === DEFAULT_FONT_FAMILY ? " selected" : ""}`}
            style={{ fontFamily: fontFamilyStack(DEFAULT_FONT_FAMILY) }}
          >
            <input
              type="radio"
              name="default-font"
              value={DEFAULT_FONT_FAMILY}
              checked={fontFamily === DEFAULT_FONT_FAMILY}
              onChange={() => onFontFamilyChange(DEFAULT_FONT_FAMILY)}
            />
            <span>System default</span>
            <span className="font-option-preview" aria-hidden="true">Aa</span>
          </label>
        ) : null}
        {!fontsLoading && !selectedFontIsInstalled && fontFamily !== DEFAULT_FONT_FAMILY ? (
          <label
            className="font-option selected"
            style={{ fontFamily: fontFamilyStack(fontFamily) }}
          >
            <input type="radio" name="default-font" value={fontFamily} checked readOnly />
            <span>{fontFamily} <small>Unavailable</small></span>
            <span className="font-option-preview" aria-hidden="true">Aa</span>
          </label>
        ) : null}
        {visibleFontFamilies.map((family) => (
          <label
            key={family}
            className={`font-option${fontFamily === family ? " selected" : ""}`}
            style={{ fontFamily: fontFamilyStack(family) }}
          >
            <input
              type="radio"
              name="default-font"
              value={family}
              checked={fontFamily === family}
              onChange={() => onFontFamilyChange(family)}
            />
            <span>{family}</span>
            <span className="font-option-preview" aria-hidden="true">Aa</span>
          </label>
        ))}
        {fontsLoading ? <p className="font-picker-status">Loading installed fonts…</p> : null}
        {fontLoadFailed ? (
          <p className="font-picker-status">Installed fonts couldn’t be loaded. System default remains available.</p>
        ) : null}
        {!fontsLoading && !fontLoadFailed && !showSystemFont
          && visibleFontFamilies.length === 0 && normalizedFontQuery ? (
          <p className="font-picker-status">No installed fonts match “{fontQuery.trim()}”.</p>
        ) : null}
      </div>
    </section>
  );
}

function ReadingSettings({
  autoReadDelaySeconds,
  onAutoReadDelayChange,
}: {
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
}) {
  return (
    <section className="settings-section" aria-label="Reading">
      <h3>Mark as read</h3>
      <label className="settings-field settings-field-inline">
        <span>After opening a conversation</span>
        <div className="settings-row">
          <input
            type="number"
            min={MIN_AUTO_READ_DELAY_SECONDS}
            max={MAX_AUTO_READ_DELAY_SECONDS}
            step="1"
            value={autoReadDelaySeconds}
            aria-label="Auto-read delay"
            onChange={(event) => {
              if (event.target.value === "") return;
              const next = Number(event.target.value);
              if (Number.isFinite(next)) onAutoReadDelayChange(next);
            }}
          />
          <span>seconds</span>
        </div>
      </label>
      <p className="settings-hint">
        Set to 0 to mark conversations read immediately. The timer resets when you open a different conversation.
      </p>
    </section>
  );
}

function AccountsSettings({
  authStatus,
  accounts,
  onAdd,
  onRemove,
  onReconnect,
  onSetDisplayName,
  onSetColor,
  onReorder,
}: {
  authStatus: AuthStatus | null;
  accounts: Account[];
  onAdd(): Promise<void>;
  onRemove(email: string): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onSetDisplayName(email: string, displayName: string | null): Promise<void>;
  onSetColor(email: string, color: string): Promise<void>;
  onReorder(emails: string[]): Promise<void>;
}) {
  const [busyEmail, setBusyEmail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyEmail(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyEmail(null));
  };

  const move = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= accounts.length) return;
    const next = [...accounts];
    [next[index], next[target]] = [next[target]!, next[index]!];
    act(accounts[index]!.email, () => onReorder(next.map((account) => account.email)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Mail Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Connected mail accounts</h3>
          <p>
            ThreeStrands keeps accounts separate and merges their inboxes by default.
            Use the sidebar or command palette to filter to one account.
          </p>
        </div>
        <button
          type="button"
          className="primary-action settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => act("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Add account"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="accounts-config-notice">
          <AlertCircle size={16} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>
              Set <code>THREESTRANDS_GOOGLE_CLIENT_ID</code> and{" "}
              <code>THREESTRANDS_GOOGLE_CLIENT_SECRET</code> from a Google Desktop
              app credential, then restart ThreeStrands.
            </p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><Mail size={18} /></span>
          <strong>No accounts connected</strong>
          <p>Add a Gmail account to start syncing mail on this device.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account, index) => (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar" aria-hidden="true" style={{ background: account.color }}>
                  {(account.displayName ?? account.email).charAt(0).toUpperCase()}
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong>{account.displayName ?? account.email}</strong>
                    <span className={`account-status ${account.status}`}>
                      {account.status === "needs_reauth" ? <AlertCircle size={13} /> : <CheckCircle2 size={13} />}
                      {account.status === "needs_reauth" ? "Needs reconnect" : "Connected"}
                    </span>
                  </div>
                  <span className="account-card-email">
                    {account.displayName ? `${account.email} · ` : null}
                    {account.lastSyncedAt ? `Last synced ${timeFormatter.format(new Date(account.lastSyncedAt))}` : "Not synced yet"}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <AccountSenderNameInput
                  email={account.email}
                  name={account.displayName}
                  onCommit={(name) =>
                    onSetDisplayName(account.email, name).catch((reason: unknown) => {
                      setError(reason instanceof Error ? reason.message : String(reason));
                      throw reason;
                    })
                  }
                />
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button
                      type="button"
                      aria-label={`Move ${account.email} up`}
                      disabled={index === 0 || busyEmail !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={14} />
                    </button>
                    <button
                      type="button"
                      aria-label={`Move ${account.email} down`}
                      disabled={index === accounts.length - 1 || busyEmail !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={14} />
                    </button>
                  </span>
                  <label className="account-color-swatch" title={`Color for ${account.email}`}>
                    <AccountColorInput
                      email={account.email}
                      color={account.color}
                      onCommit={(color) =>
                        onSetColor(account.email, color).catch((reason: unknown) => {
                          setError(reason instanceof Error ? reason.message : String(reason));
                          throw reason;
                        })
                      }
                    />
                  </label>
                  {account.status === "needs_reauth" ? (
                    <button
                      type="button"
                      className="account-action-button"
                      disabled={busyEmail !== null}
                      onClick={() => act(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => act(account.email, () => onRemove(account.email))}
                  >
                    Disconnect
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="accounts-footnote">
        Disconnecting removes this account and its local ThreeStrands cache. Gmail and the account itself are not changed.
      </p>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function CalendarAccountsSettings({
  authStatus,
  accounts,
  onAdd,
  onReconnect,
  onRemove,
}: {
  authStatus: AuthStatus | null;
  accounts: CalendarAccount[];
  onAdd(): Promise<void>;
  onReconnect(email: string): Promise<void>;
  onRemove(email: string): Promise<void>;
}) {
  const [busyEmail, setBusyEmail] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyEmail(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyEmail(null));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Calendar Accounts">
      <div className="accounts-manager-header">
        <div>
          <h3>Google Calendar</h3>
          <p>
            Calendar access is connected separately from mail and is read-only.
            Each account gets its own Calendar consent and keychain credential.
          </p>
        </div>
        <button
          type="button"
          className="primary-action settings-add-account"
          disabled={busyEmail !== null}
          onClick={() => act("__add__", onAdd)}
        >
          <Plus size={15} />
          {busyEmail === "__add__" ? "Waiting for Google…" : "Connect calendar"}
        </button>
      </div>
      {accounts.length === 0 && authStatus && !authStatus.configured ? (
        <div className="accounts-config-notice">
          <AlertCircle size={16} />
          <div>
            <strong>Google OAuth is not configured</strong>
            <p>Configure the Google Desktop app credentials used for mail, then restart ThreeStrands.</p>
          </div>
        </div>
      ) : null}
      {accounts.length === 0 ? (
        <div className="accounts-empty">
          <span className="accounts-empty-icon"><CalendarDays size={18} /></span>
          <strong>No calendars connected</strong>
          <p>Connect Google Calendar to use the T shortcut and see your live schedule.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account) => (
            <li className="account-card" key={account.email}>
              <div className="account-card-row">
                <span className="account-card-avatar calendar-account-avatar" aria-hidden="true">
                  <CalendarDays size={18} />
                </span>
                <div className="account-card-identity">
                  <div className="account-card-heading">
                    <strong>{account.email}</strong>
                    <span className={`account-status ${account.status}`}>
                      {account.status === "needs_reauth" ? <AlertCircle size={13} /> : <CheckCircle2 size={13} />}
                      {account.status === "needs_reauth" ? "Needs reconnect" : "Connected"}
                    </span>
                  </div>
                  <span className="account-card-email">Read-only calendar access</span>
                </div>
                <span className="accounts-list-actions">
                  {account.status === "needs_reauth" ? (
                    <button
                      type="button"
                      className="account-action-button"
                      disabled={busyEmail !== null}
                      onClick={() => act(account.email, () => onReconnect(account.email))}
                    >
                      Reconnect
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyEmail !== null}
                    onClick={() => act(account.email, () => onRemove(account.email))}
                  >
                    Disconnect
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

/** Resolves a label id to its display name by scanning every account's label
 * catalog rather than storing the name on the split inbox — ids are stable,
 * but the label could be renamed after the split inbox is created. */
function describeSplitInboxRule(splitInbox: SplitInbox, labelsByAccount: Record<string, Label[]>): string {
  switch (splitInbox.matchKind) {
    case "domain":
      return `Sending domain: ${splitInbox.matchValue}`;
    case "pattern":
      return `Address contains: ${splitInbox.matchValue}`;
    case "label": {
      // Gmail label ids are opaque per account, so only the split's own
      // account's catalog can resolve this id to a real name.
      const label = (labelsByAccount[splitInbox.accountId] ?? []).find(
        (candidate) => candidate.id === splitInbox.matchValue,
      );
      return `Label: ${label ? formatLabelName(label) : splitInbox.matchValue}`;
    }
  }
}

function SplitInboxNameInput({
  splitInbox,
  onCommit,
}: {
  splitInbox: SplitInbox;
  onCommit(name: string): void;
}) {
  const [value, setValue] = useState(splitInbox.name);
  const normalized = value.trim();

  useEffect(() => setValue(splitInbox.name), [splitInbox.name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (!normalized || normalized === splitInbox.name) return;
        onCommit(normalized);
      }}
    >
      <input
        aria-label={`Name for ${splitInbox.name}`}
        value={value}
        maxLength={200}
        onChange={(event) => setValue(event.target.value)}
      />
      <button type="submit" disabled={!normalized || normalized === splitInbox.name}>
        Save name
      </button>
    </form>
  );
}

function SplitInboxesSettings({
  splitInboxes,
  accounts,
  activeAccountId,
  labelsByAccount,
  onCreate,
  onRename,
  onDelete,
  onReorder,
}: {
  splitInboxes: SplitInbox[];
  accounts: Account[];
  activeAccountId: string | null;
  labelsByAccount: Record<string, Label[]>;
  onCreate(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onRename(id: string, name: string): Promise<void>;
  onDelete(id: string): Promise<void>;
  onReorder(ids: string[]): Promise<void>;
}) {
  const [name, setName] = useState("");
  const [matchKind, setMatchKind] = useState<SplitInboxMatchKind>("domain");
  const [matchValue, setMatchValue] = useState("");
  const [accountId, setAccountId] = useState(() => activeAccountId ?? accounts[0]?.email ?? "");
  const [creating, setCreating] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  // A split inbox belongs to one account, so only that account's labels are
  // valid matches for it.
  const labelOptions = (labelsByAccount[accountId] ?? []).filter((label) => label.kind === "user");

  const act = (busyKey: string, operation: () => Promise<void>) => {
    setBusyId(busyKey);
    setError(null);
    void operation()
      .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
      .finally(() => setBusyId(null));
  };

  const move = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    if (target < 0 || target >= splitInboxes.length) return;
    const next = [...splitInboxes];
    [next[index], next[target]] = [next[target]!, next[index]!];
    act(splitInboxes[index]!.id, () => onReorder(next.map((splitInbox) => splitInbox.id)));
  };

  return (
    <section className="settings-section accounts-manager" aria-label="Split Inboxes">
      <div className="accounts-manager-header">
        <div>
          <h3>Split inboxes</h3>
          <p>
            Show a filtered slice of your inbox as its own view in the sidebar —
            for example, every message from one client's sending domain.
          </p>
        </div>
      </div>
      <form
        className="split-inbox-add"
        onSubmit={(event) => {
          event.preventDefault();
          const normalizedName = name.trim();
          const normalizedValue = matchValue.trim();
          if (!normalizedName || !normalizedValue || !accountId || creating) return;
          setCreating(true);
          setError(null);
          void onCreate(normalizedName, matchKind, normalizedValue, accountId)
            .then(() => {
              setName("");
              setMatchValue("");
            })
            .catch((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)))
            .finally(() => setCreating(false));
        }}
      >
        <input
          value={name}
          placeholder="Name (e.g. Acme Corp)"
          aria-label="Split inbox name"
          onChange={(event) => setName(event.target.value)}
        />
        <select
          value={accountId}
          aria-label="Account"
          onChange={(event) => {
            setAccountId(event.target.value);
            setMatchValue("");
          }}
        >
          {accounts.map((account) => (
            <option key={account.email} value={account.email}>{account.email}</option>
          ))}
        </select>
        <select
          value={matchKind}
          aria-label="Match by"
          onChange={(event) => {
            setMatchKind(event.target.value as SplitInboxMatchKind);
            setMatchValue("");
          }}
        >
          <option value="domain">Sending domain</option>
          <option value="label">Label</option>
          <option value="pattern">Address contains</option>
        </select>
        {matchKind === "label" ? (
          <select value={matchValue} aria-label="Label" onChange={(event) => setMatchValue(event.target.value)}>
            <option value="" disabled>Choose a label</option>
            {labelOptions.map((label) => (
              <option key={label.id} value={label.id}>{formatLabelName(label)}</option>
            ))}
          </select>
        ) : (
          <input
            value={matchValue}
            placeholder={matchKind === "domain" ? "acme.com" : "boss@"}
            aria-label={matchKind === "domain" ? "Sending domain" : "Address pattern"}
            onChange={(event) => setMatchValue(event.target.value)}
          />
        )}
        <button type="submit" className="primary-action" disabled={creating || !name.trim() || !matchValue.trim() || !accountId}>
          <Plus size={15} />
          {creating ? "Adding…" : "Add split inbox"}
        </button>
      </form>
      {splitInboxes.length === 0 ? (
        <div className="accounts-empty">
          <strong>No split inboxes yet</strong>
          <p>Add one above to see a filtered slice of your inbox in the sidebar.</p>
        </div>
      ) : (
        <ul className="accounts-list">
          {splitInboxes.map((splitInbox, index) => (
            <li className="account-card" key={splitInbox.id}>
              <div className="account-card-row">
                <span className="account-card-avatar split-inbox-avatar" aria-hidden="true">
                  {splitInbox.name.charAt(0).toUpperCase()}
                </span>
                <div className="account-card-identity">
                  <SplitInboxNameInput
                    splitInbox={splitInbox}
                    onCommit={(nextName) => act(splitInbox.id, () => onRename(splitInbox.id, nextName))}
                  />
                  <span className="account-card-email">
                    {describeSplitInboxRule(splitInbox, labelsByAccount)} — {splitInbox.accountId}
                  </span>
                </div>
              </div>
              <div className="account-card-controls">
                <span className="accounts-list-actions">
                  <span className="account-reorder">
                    <button
                      type="button"
                      aria-label={`Move ${splitInbox.name} up`}
                      disabled={index === 0 || busyId !== null}
                      onClick={() => move(index, -1)}
                    >
                      <ChevronUp size={14} />
                    </button>
                    <button
                      type="button"
                      aria-label={`Move ${splitInbox.name} down`}
                      disabled={index === splitInboxes.length - 1 || busyId !== null}
                      onClick={() => move(index, 1)}
                    >
                      <ChevronDown size={14} />
                    </button>
                  </span>
                  <button
                    type="button"
                    className="account-action-button danger-action"
                    disabled={busyId !== null}
                    onClick={() => act(splitInbox.id, () => onDelete(splitInbox.id))}
                  >
                    Delete
                  </button>
                </span>
              </div>
            </li>
          ))}
        </ul>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function AccountSenderNameInput({
  email,
  name,
  onCommit,
}: {
  email: string;
  name: string | null;
  onCommit(name: string | null): Promise<void>;
}) {
  const [value, setValue] = useState(name ?? "");
  const [saving, setSaving] = useState(false);
  const normalized = value.trim();
  const saved = name?.trim() ?? "";

  useEffect(() => setValue(name ?? ""), [name]);

  return (
    <form
      className="account-sender-name"
      onSubmit={(event) => {
        event.preventDefault();
        if (saving || normalized === saved) return;
        setSaving(true);
        void onCommit(normalized || null).finally(() => setSaving(false));
      }}
    >
      <input
        aria-label={`Sender name for ${email}`}
        value={value}
        maxLength={200}
        placeholder="Sender name"
        disabled={saving}
        onChange={(event) => setValue(event.target.value)}
      />
      <button type="submit" disabled={saving || normalized === saved}>
        {saving ? "Saving…" : "Save name"}
      </button>
    </form>
  );
}

/**
 * A color swatch that saves on its own debounced schedule instead of on
 * every drag tick. `<input type="color">` fires `onChange` continuously
 * while the native picker is open, not just once on commit — driving that
 * straight into a save-and-disable cycle (the previous implementation) could
 * disable the input mid-drag and drop the rest of the gesture, so only the
 * first flicker of color ever got saved. Local `value` gives smooth
 * dragging; `color` (the saved value) is only adopted once no locally
 * committed save is still in flight, so a slow save can't snap the swatch
 * back to a stale color out from under the user.
 */
function AccountColorInput({
  email,
  color,
  onCommit,
}: {
  email: string;
  color: string;
  onCommit(color: string): Promise<void>;
}) {
  const [value, setValue] = useState(color);
  const pendingCount = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (pendingCount.current === 0) setValue(color);
  }, [color]);

  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);

  return (
    <input
      type="color"
      aria-label={`Color for ${email}`}
      value={value}
      onChange={(event) => {
        const next = event.target.value;
        setValue(next);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => {
          pendingCount.current++;
          onCommit(next)
            .catch(() => {})
            .finally(() => { pendingCount.current--; });
        }, 200);
      }}
    />
  );
}

function AiProviderSettings({ onChange }: { onChange?: () => void }) {
  const [provider, setProvider] = useState(readAiProvider);
  const [model, setModel] = useState(readAiModel);
  const [endpoint, setEndpoint] = useState(readAiEndpoint);
  const [features, setFeatures] = useState<AiFeatureFlags>(readAiFeatures);
  const [keyConfigured, setKeyConfigured] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void isAiApiKeyConfigured().then(setKeyConfigured);
  }, []);

  const updateFeature = (flag: keyof AiFeatureFlags, value: boolean) => {
    setFeatures((current) => {
      const next = { ...current, [flag]: value };
      saveAiFeatures(next);
      return next;
    });
    onChange?.();
  };

  return (
    <section className="settings-section" aria-label="AI provider">
      <p className="settings-hint">
        Disabled by default. ThreeStrands only sends thread content to your chosen
        provider for the features you turn on below, using your own API key.
      </p>

      <label className="settings-field">
        <span>Provider</span>
        <select
          value={provider}
          onChange={(event) => {
            const next = event.target.value as AiProvider;
            setProvider(next);
            saveAiProvider(next);
            onChange?.();
          }}
        >
          {AI_PROVIDER_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </label>

      {provider !== "none" ? (
        <>
          <label className="settings-field">
            <span>Model</span>
            <input
              value={model}
              placeholder={AI_MODEL_PLACEHOLDERS[provider]}
              onChange={(event) => {
                setModel(event.target.value);
                saveAiModel(event.target.value);
              }}
            />
          </label>

          {provider === "custom" ? (
            <label className="settings-field">
              <span>Endpoint URL</span>
              <input
                value={endpoint}
                placeholder="https://api.example.com/v1"
                onChange={(event) => {
                  setEndpoint(event.target.value);
                  saveAiEndpoint(event.target.value);
                }}
              />
            </label>
          ) : null}

          <label className="settings-field">
            <span>API key</span>
            <input
              type="password"
              value={keyInput}
              placeholder={keyConfigured ? "Saved to keychain" : "Paste API key"}
              onChange={(event) => setKeyInput(event.target.value)}
            />
          </label>
          <div className="settings-row">
            <button
              disabled={busy || !keyInput.trim()}
              onClick={() => {
                setBusy(true);
                void setAiApiKey(keyInput)
                  .then(() => {
                    setKeyInput("");
                    return isAiApiKeyConfigured();
                  })
                  .then(setKeyConfigured)
                  .then(() => onChange?.())
                  .finally(() => setBusy(false));
              }}
            >
              Save key
            </button>
            <button
              disabled={busy || !keyConfigured}
              onClick={() => {
                setBusy(true);
                void clearAiApiKey()
                  .then(() => isAiApiKeyConfigured())
                  .then(setKeyConfigured)
                  .then(() => onChange?.())
                  .finally(() => setBusy(false));
              }}
            >
              Remove key
            </button>
          </div>
          <span className="settings-hint">
            Stored in your OS keychain, never in the mail database.
          </span>

          <h3>Features</h3>
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={features.draftAssist}
              onChange={(event) => updateFeature("draftAssist", event.target.checked)}
            />
            Draft assist
          </label>
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={features.summarize}
              onChange={(event) => updateFeature("summarize", event.target.checked)}
            />
            Thread summaries
          </label>
        </>
      ) : null}
    </section>
  );
}

function PrivacySettings({
  loadRemoteImages,
  onLoadRemoteImagesChange,
}: {
  loadRemoteImages: boolean;
  onLoadRemoteImagesChange(value: boolean): void;
}) {
  const [reporting, setReporting] = useState(crashReportingEnabled);
  const [reportCount, setReportCount] = useState(() => localCrashReports().length);
  const [retentionDays, setRetentionDaysState] = useState<number | null>(null);

  useEffect(() => {
    void getRetentionDays().then(setRetentionDaysState);
  }, []);

  return (
    <section className="settings-section" aria-label="Privacy">
      <h3>Local storage</h3>
      <label className="settings-field">
        <span>Keep mail on this device for</span>
        <select
          value={retentionDays === null ? "forever" : String(retentionDays)}
          onChange={(event) => {
            const next = event.target.value === "forever" ? null : Number(event.target.value);
            setRetentionDaysState(next);
            void setRetentionDays(next);
          }}
        >
          {RETENTION_OPTIONS.map((option) => (
            <option key={option.label} value={option.value === null ? "forever" : String(option.value)}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
      <span className="settings-hint">
        Mail older than this is removed from ThreeStrands's local cache to keep the
        database from growing without bound. It stays on the server.
      </span>

      <h3>Message images</h3>
      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={loadRemoteImages}
          onChange={(event) => onLoadRemoteImagesChange(event.target.checked)}
        />
        Load remote images automatically
      </label>
      <span className="settings-hint">
        When disabled, images stay blocked until you choose Load images in a message.
      </span>

      <h3>Crash reports</h3>
      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={reporting}
          onChange={(event) => {
            setReporting(event.target.checked);
            setCrashReportingEnabled(event.target.checked);
          }}
        />
        Share sanitized crash reports
      </label>
      <span className="settings-hint">
        Disabled by default. Email addresses and URLs are redacted.{" "}
        Policy: <code>docs/crash-reporting.md</code>
      </span>
      <button
        disabled={reportCount === 0}
        onClick={() => {
          clearLocalCrashReports();
          setReportCount(0);
        }}
      >
        Clear {reportCount} local {reportCount === 1 ? "report" : "reports"}
      </button>
    </section>
  );
}

function DataTransferSettings({
  onImported,
}: {
  onImported(result: SettingsImportResult): Promise<void>;
}) {
  const [exportPassword, setExportPassword] = useState("");
  const [exportConfirmation, setExportConfirmation] = useState("");
  const [importPassword, setImportPassword] = useState("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const isDesktop = "__TAURI_INTERNALS__" in window;
  const passwordsMatch = exportPassword.length >= 8 && exportPassword === exportConfirmation;

  const showError = (error: unknown) => {
    setMessage(error instanceof Error ? error.message : String(error));
  };

  return (
    <section className="settings-section" aria-label="Data transfer">
      <h3>Export settings and accounts</h3>
      <p className="settings-hint">
        Creates a password-encrypted file containing your preferences, account
        list, Split Inboxes, and retention setting. Mail, OAuth credentials,
        API keys, and other keychain secrets are never exported.
      </p>
      <label className="settings-field">
        <span>Export password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportPassword}
          onChange={(event) => setExportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <label className="settings-field">
        <span>Confirm password</span>
        <input
          type="password"
          autoComplete="new-password"
          value={exportConfirmation}
          onChange={(event) => setExportConfirmation(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <button
        type="button"
        className="settings-transfer-action"
        disabled={!isDesktop || !passwordsMatch || busy !== null}
        onClick={() => {
          setBusy("export");
          setMessage(null);
          void exportSettings(exportPassword)
            .then((path) => {
              if (path) {
                setMessage(`Settings exported to ${path}`);
                setExportPassword("");
                setExportConfirmation("");
              }
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Download size={15} aria-hidden="true" />
        {busy === "export" ? "Exporting…" : "Export encrypted settings"}
      </button>

      <h3>Import settings and accounts</h3>
      <p className="settings-hint">
        Importing replaces preferences and Split Inboxes from this installation.
        Existing connected accounts stay connected. Other imported accounts
        appear as “Connect on this device” and require Google authorization.
      </p>
      <label className="settings-field">
        <span>Export password</span>
        <input
          type="password"
          autoComplete="current-password"
          value={importPassword}
          onChange={(event) => setImportPassword(event.target.value)}
          disabled={!isDesktop || busy !== null}
        />
      </label>
      <button
        type="button"
        className="settings-transfer-action"
        disabled={!isDesktop || importPassword.length < 8 || busy !== null}
        onClick={() => {
          setBusy("import");
          setMessage(null);
          void importSettings(importPassword)
            .then(async (result) => {
              if (!result) return;
              await onImported(result);
            })
            .catch(showError)
            .finally(() => setBusy(null));
        }}
      >
        <Upload size={15} aria-hidden="true" />
        {busy === "import" ? "Importing…" : "Choose encrypted settings file"}
      </button>
      {!isDesktop ? (
        <p className="settings-hint" role="status">
          Settings transfer is available in the ThreeStrands desktop app.
        </p>
      ) : null}
      {message ? <p className="settings-hint" role="status">{message}</p> : null}
    </section>
  );
}

function UnsubscribeConfirm({
  message,
  onClose,
  onConfirm,
}: {
  message: Message;
  onClose(): void;
  onConfirm(): Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const method = message.unsubscribe?.methods[0];
  const sender = parseAddress(message.sender);
  const action = method === "oneClick"
    ? "Send one-click request"
    : method === "mailto"
      ? "Open unsubscribe email"
      : "Open unsubscribe page";

  return (
    <Modal title="Unsubscribe" className="unsubscribe-modal" onClose={onClose}>
      <div className="unsubscribe-content">
        <p>
          Unsubscribe from <strong>{sender.name}</strong>?
          {message.unsubscribe?.listId ? <span className="unsubscribe-list">{message.unsubscribe.listId}</span> : null}
        </p>
        <p className="unsubscribe-explanation">
          {method === "oneClick"
            ? "ThreeStrands will send the sender's one-click request without opening a web page."
            : method === "mailto"
              ? "ThreeStrands will open a new email in your default mail handler. You will still need to send it."
              : "ThreeStrands will open the sender's unsubscribe page in your default browser."}
        </p>
        <div className="unsubscribe-actions">
          <button type="button" onClick={onClose} disabled={busy}>Cancel</button>
          <button
            type="button"
            className="primary-action"
            disabled={busy || !method}
            onClick={() => {
              setBusy(true);
              void onConfirm().finally(() => setBusy(false));
            }}
          >
            {busy ? "Working…" : action}
          </button>
        </div>
      </div>
    </Modal>
  );
}

function ImageLightbox({ src, onClose }: { src: string; onClose(): void }) {
  useEscapeDismiss(onClose);
  return (
    <div className="modal-backdrop lightbox-backdrop" role="presentation" onMouseDown={onClose}>
      <button type="button" className="lightbox-close" aria-label="Close" onClick={onClose}>
        <X size={20} />
      </button>
      <img src={src} alt="" className="lightbox-image" onMouseDown={(event) => event.stopPropagation()} />
    </div>
  );
}

function Modal({
  className,
  children,
  onClose,
  title,
}: {
  className?: string;
  children: React.ReactNode;
  onClose(): void;
  title: string;
}) {
  useEscapeDismiss(onClose);
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <div
        className={`modal${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header><h2>{title}</h2><button aria-label="Close" onClick={onClose}><X size={18} /></button></header>
        {children}
      </div>
    </div>
  );
}
