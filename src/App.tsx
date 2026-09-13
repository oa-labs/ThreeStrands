import {
  Archive,
  Check,
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
  Mail,
  Mails,
  MailOpen,
  Moon,
  Sun,
  Pencil,
  RefreshCw,
  RotateCcw,
  Search,
  Settings as SettingsIcon,
  ShieldAlert,
  Square,
  Star,
  Tag,
  Trash2,
  Unlink,
  X,
} from "lucide-react";
import {
  type CSSProperties,
  useCallback,
  useEffect,
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
import { formattingShortcuts } from "./richText";
import type {
  Account,
  AuthStatus,
  Label,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
  Message,
} from "./domain";
import { InboxResizeHandle, useInboxWidth } from "./InboxResizeHandle";
import { DraftsList, OutboxList, useCorrespondence } from "./useCorrespondence";
import { SafeMessage } from "./SafeMessage";
import {
  applyFontScale,
  changeFontScale,
  FONT_SCALE_STEP,
  MAX_FONT_SCALE,
  MIN_FONT_SCALE,
  readFontScale,
  saveFontScale,
} from "./fontScale";

import { applyTheme, effectiveTheme, readTheme, saveTheme, type Theme } from "./theme";
import { applyFontFamily, FONT_FAMILY_OPTIONS, readFontFamily, saveFontFamily, type FontFamily } from "./settings";
import {
  AI_PROVIDER_OPTIONS,
  clearAiApiKey,
  isAiApiKeyConfigured,
  readAiEndpoint,
  readAiFeatures,
  readAiModel,
  readAiProvider,
  saveAiEndpoint,
  saveAiFeatures,
  saveAiModel,
  saveAiProvider,
  setAiApiKey,
  type AiFeatureFlags,
  type AiProvider,
} from "./aiSettings";
import { useEscapeDismiss } from "./useEscapeDismiss";

type SettingsSection = "appearance" | "account" | "accounts" | "ai" | "privacy";

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

const SEARCH_PAGE_SIZE = 50;

const MAILBOX_TITLES: Record<MailboxKind, string> = {
  inbox: "Inbox",
  allMail: "All Mail",
  trash: "Trash",
  drafts: "Drafts",
  outbox: "Outbox",
};

// Matches the \u{1}/\u{2} markers the backend's FTS5 `snippet()` call wraps
// hits in (see search_threads in src-tauri/src/db.rs). Rendered as React
// elements rather than HTML so a match can never inject markup.
const MATCH_START = "";
const MATCH_END = "";

function HighlightedSnippet({ thread }: { thread: Thread }) {
  const raw = thread.matchSnippet;
  if (!raw) return <>{thread.snippet}</>;
  const segments = raw.split(MATCH_START);
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
  return [...threads].sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt));
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
      const fontShortcut = (event.metaKey || event.ctrlKey) && ["=", "+", "-"].includes(event.key);
      const dialog = document.querySelector('[role="dialog"]');
      const allowsMailboxNavigation = dialog?.classList.contains("correspondence-list");
      if (!sendShortcut && !fontShortcut && (isEditableTarget(event.target) || (dialog && !allowsMailboxNavigation))) {
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
  const [theme, setTheme] = useState(readTheme);
  const [fontScale, setFontScale] = useState(readFontScale);
  const [fontFamily, setFontFamily] = useState(readFontFamily);
  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => applyFontScale(fontScale), [fontScale]);
  useEffect(() => applyFontFamily(fontFamily), [fontFamily]);
  useEffect(() => {
    if (theme !== "system") return;
    const query = window.matchMedia?.("(prefers-color-scheme: light)");
    if (!query) return;
    const handleChange = () => applyTheme("system");
    query.addEventListener("change", handleChange);
    return () => query.removeEventListener("change", handleChange);
  }, [theme]);
  const effectiveThemeValue = effectiveTheme(theme);
  const toggleTheme = () => {
    const next = effectiveThemeValue === "dark" ? "light" : "dark";
    saveTheme(next);
    setTheme(next);
  };
  const adjustFontScale = useCallback((direction: 1 | -1) => {
    setFontScale((current) => saveFontScale(changeFontScale(current, direction)));
  }, []);
  const [threads, setThreads] = useState<Thread[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [checkedIds, setCheckedIds] = useState<Set<string>>(new Set());
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const correspondence = useCorrespondence(accounts, detail?.messages.at(-1)?.id, detail?.thread.accountId);
  const [query, setQuery] = useState("");
  const [includeArchived, setIncludeArchived] = useState(false);
  const [hasMoreResults, setHasMoreResults] = useState(false);
  const [loading, setLoading] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false);
  const [unsubscribeMessageId, setUnsubscribeMessageId] = useState<string | null>(null);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [labelTargetIds, setLabelTargetIds] = useState<string[] | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");
  const [labels, setLabels] = useState<Label[]>([]);
  const [authStatus, setAuthStatus] = useState<AuthStatus | null>(null);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [activeAccountId, setActiveAccountId] = useState<string | null>(null);
  const [mailbox, setMailbox] = useState<MailboxKind>("inbox");
  const isThreadMailbox = mailbox === "inbox" || mailbox === "allMail" || mailbox === "trash";
  const accountsRequest = useRef(0);
  const refreshAccounts = useCallback(() => {
    // Guards against an earlier-issued refresh resolving after a later one
    // (e.g. two account edits in quick succession) and clobbering it with
    // stale data.
    const requestId = ++accountsRequest.current;
    return mailClient
      .listAccounts()
      .then((next) => {
        if (requestId === accountsRequest.current) setAccounts(next);
      })
      .catch(() => {
        if (requestId === accountsRequest.current) setAccounts([]);
      });
  }, []);
  const [notice, setNotice] = useNotice();
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
    const box = mailboxOverride ?? mailbox;
    if (box === "drafts" || box === "outbox") {
      setThreads([]);
      setSelectedId(null);
      setHasMoreResults(false);
      return;
    }
    const trimmed = search.trim();
    const accountId = (accountOverride !== undefined ? accountOverride : activeAccountId) ?? undefined;
    const next = box === "inbox" && trimmed
      ? await mailClient.searchThreads({
          query: trimmed,
          limit: SEARCH_PAGE_SIZE,
          includeArchived,
        }, accountId)
      : box === "allMail"
        ? await mailClient.listAllMail(accountId)
        : box === "trash"
          ? await mailClient.listTrash(accountId)
          : await mailClient.listThreads(accountId);
    setThreads(next);
    setHasMoreResults(box === "inbox" && trimmed ? next.length === SEARCH_PAGE_SIZE : false);
    setSelectedId((current) =>
      current && next.some((thread) => thread.id === current)
        ? current
        : (next[0]?.id ?? null),
    );
  }, [includeArchived, activeAccountId, mailbox]);

  const loadMoreResults = useCallback(async () => {
    const trimmed = query.trim();
    if (!trimmed) return;
    const next = await mailClient.searchThreads({
      query: trimmed,
      limit: SEARCH_PAGE_SIZE,
      offset: threads.length,
      includeArchived,
    }, activeAccountId ?? undefined);
    setThreads((current) => [...current, ...next]);
    setHasMoreResults(next.length === SEARCH_PAGE_SIZE);
  }, [query, threads.length, includeArchived, activeAccountId]);

  useEffect(() => {
    if (correspondence.sentCount > 0) void loadThreads(query);
  }, [correspondence.sentCount, loadThreads]);

  useEffect(() => {
    Promise.all([loadThreads(""), mailClient.syncStatus(), mailClient.googleAuthStatus()])
      .then(([, status, auth]) => {
        setSyncStatus(status);
        setAuthStatus(auth);
        void mailClient.listLabels().then(setLabels).catch(() => setLabels([]));
        refreshAccounts();
      })
      .finally(() => setLoading(false));
  }, [loadThreads]);

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return;
    }
    mailClient.getThread(selectedId).then(setDetail);
  }, [selectedId, threads]);

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
    const timeout = window.setTimeout(() => void loadThreads(query), 180);
    return () => window.clearTimeout(timeout);
  }, [query, loadThreads]);

  useEffect(() => {
    // The toggle itself is only reachable while searching; reset it with the
    // search box so a stale "include archived" flag can't linger over into
    // the plain inbox view, where the backend always excludes archived mail.
    if (!query.trim()) setIncludeArchived(false);
  }, [query]);

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
  });

  const mutateIds = useCallback(async (ids: string[], template: MutationTemplate): Promise<CommandResult> => {
    const targetIds = ids.filter((id) => threads.some((thread) => thread.id === id));
    if (targetIds.length === 0) return {};
    const previous = new Map(
      threads.filter((thread) => targetIds.includes(thread.id)).map((thread) => [thread.id, thread] as const),
    );
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

    if (removesFromView && selectedId && targetIds.includes(selectedId)) {
      const currentIndex = threads.findIndex((thread) => thread.id === selectedId);
      const remaining = threads.filter((thread) => !targetIds.includes(thread.id));
      const nextIndex = Math.min(currentIndex, remaining.length - 1);
      setSelectedId(remaining[nextIndex]?.id ?? null);
    }

    setCheckedIds((current) => {
      if (current.size === 0) return current;
      const next = new Set(current);
      targetIds.forEach((id) => next.delete(id));
      return next.size === current.size ? current : next;
    });

    const settled = await Promise.allSettled(
      targetIds.map((id) => mailClient.mutateThread(buildThreadMutation(id, template))),
    );
    const failedIds = targetIds.filter((_, index) => settled[index].status === "rejected");
    const succeededIds = targetIds.filter((id) => !failedIds.includes(id));

    if (failedIds.length > 0) {
      setThreads((current) => {
        const restored = failedIds
          .map((id) => previous.get(id))
          .filter((thread): thread is Thread => Boolean(thread));
        return sortByRecency([...current.filter((thread) => !failedIds.includes(thread.id)), ...restored]);
      });
    }

    if (template.kind !== "archive" && template.kind !== "trash" && template.kind !== "spam") {
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
        await Promise.allSettled(
          succeededIds.map((id) => mailClient.mutateThread(buildThreadMutation(id, undoTemplate))),
        );
        await loadThreads(query);
      },
    };
  }, [threads, includeArchived, mailbox, selectedId, loadThreads, query, labels, setNotice]);

  const selected = threads.find((thread) => thread.id === selectedId) ?? null;
  const selectedIndex = threads.findIndex((thread) => thread.id === selectedId);
  const latestMessage = detail?.messages.at(-1) ?? null;
  const canUnsubscribe = Boolean(latestMessage?.unsubscribe?.methods.length);
  const unsubscribeMessage = detail?.messages.find((message) => message.id === unsubscribeMessageId) ?? null;

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

  const context = useMemo<CommandContext>(() => ({
    ...correspondence.context,
    mailbox,
    selectedId,
    selectedArchived: selected?.archived ?? false,
    selectedTrashed: selected?.trashed ?? false,
    canUnsubscribe,
    openInbox: () => {
      correspondence.context.openInbox();
      setQuery("");
      setMailbox("inbox");
      void loadThreads("", undefined, "inbox");
    },
    openAllMail: () => {
      correspondence.context.openInbox();
      setQuery("");
      setMailbox("allMail");
      void loadThreads("", undefined, "allMail");
    },
    openTrash: () => {
      correspondence.context.openInbox();
      setQuery("");
      setMailbox("trash");
      void loadThreads("", undefined, "trash");
    },
    openDrafts: () => {
      correspondence.context.openDrafts();
      setQuery("");
      setMailbox("drafts");
      void loadThreads("", undefined, "drafts");
    },
    openOutbox: () => {
      correspondence.context.openOutbox();
      setQuery("");
      setMailbox("outbox");
      void loadThreads("", undefined, "outbox");
    },
    selectNext: () => {
      const next = Math.min(selectedIndex + 1, threads.length - 1);
      setSelectedId(threads[next]?.id ?? null);
    },
    selectPrevious: () => {
      const next = Math.max(selectedIndex - 1, 0);
      setSelectedId(threads[next]?.id ?? null);
    },
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
    toggleCheckedSelected: () => {
      if (!selected) return;
      setCheckedIds((current) => {
        const next = new Set(current);
        if (next.has(selected.id)) next.delete(selected.id);
        else next.add(selected.id);
        return next;
      });
    },
    focusSearch: () => searchRef.current?.focus(),
    refresh: refreshMail,
    openDiagnostics: () => setDiagnosticsOpen(true),
    openLabels: () => setLabelTargetIds(selected ? [selected.id] : null),
    openPalette: () => setPaletteOpen(true),
    openShortcutHelp: () => setShortcutHelpOpen(true),
    openSettings: () => openSettingsAt("appearance"),
    increaseFontSize: () => adjustFontScale(1),
    decreaseFontSize: () => adjustFontScale(-1),
    canUndoAction,
    undoLastAction: () => { void undoLastAction(); },
    switchAccount: (email) => {
      setActiveAccountId(email);
      void loadThreads(query, email);
    },
    showAllAccounts: () => {
      setActiveAccountId(null);
      void loadThreads(query, null);
    },
  }), [adjustFontScale, canUnsubscribe, canUndoAction, labelTargetIds, latestMessage, loadThreads, mailbox, mutateIds, openSettingsAt, query, refreshMail, selected, selectedId, selectedIndex, threads, correspondence.context, undoLastAction]);

  const executeCommand = useCallback((command: Command) => {
    void command.run(context).then((result) => {
      if (!command.undo || !result.undoAction) return;
      lastUndo.current = { command, result };
      setCanUndoAction(true);
      setNotice({
        message: result.message ?? command.title,
        undo: () => { void undoLastAction(); },
      });
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
  useShortcutHandler(context, executeCommand, accountCommands);

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

  useEffect(() => {
    if (!selectAllRef.current) return;
    selectAllRef.current.indeterminate = checkedIds.size > 0 && checkedIds.size < threads.length;
  }, [checkedIds, threads.length]);

  return (
    <main className="app-shell" style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
      <nav className="sidebar" aria-label="Mailboxes">
        <AccountSwitcher
          accounts={accounts}
          activeAccountId={activeAccountId}
          onSwitch={context.switchAccount}
          onShowAll={context.showAllAccounts}
        />
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
        <HoverTooltip label="Trash" shortcut="G T">
          <button
            className={`nav-button ${mailbox === "trash" ? "active" : ""}`}
            aria-label="Trash (g then t)"
            onClick={() => executeById("mailbox.trash")}
          >
            <Trash2 size={19} />
          </button>
        </HoverTooltip>
        <button className="nav-button" aria-label="New message (c)" title="New message (c)" onClick={() => executeById("draft.new")}><Pencil size={19} /></button>
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
        <div className="sidebar-spacer" />
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
              <span className="eyebrow">{MAILBOX_TITLES[mailbox]}</span>
              <h1>
                {mailbox === "drafts"
                  ? `${correspondence.drafts.length} drafts`
                  : mailbox === "outbox"
                    ? `${correspondence.outbox.filter((item) => item.state !== "canceled").length} outgoing`
                    : `${threads.length} conversations`}
              </h1>
            </div>
          </div>
          <button
            className="icon-button"
            aria-label="Refresh mail"
            onClick={() => executeById("mail.refresh")}
          >
            <RefreshCw size={17} className={syncStatus?.state === "syncing" ? "spin" : ""} />
          </button>
        </header>
        {isThreadMailbox && checkedIds.size > 0 ? (
          <div className="batch-toolbar" role="toolbar" aria-label="Batch actions">
            <span className="batch-count">{checkedIds.size} selected</span>
            {mailbox === "trash" ? (
              <ActionButton label="Restore" onClick={() => runOnSelection("Restore", { kind: "trash", value: false })}>
                <RotateCcw size={16} />
              </ActionButton>
            ) : (
              <>
                <ActionButton label="Archive" onClick={() => runOnSelection("Archive", { kind: "archive", value: true })}>
                  <Archive size={16} />
                </ActionButton>
                <ActionButton label="Trash" onClick={() => runOnSelection("Trash", { kind: "trash", value: true })}>
                  <Trash2 size={16} />
                </ActionButton>
                <ActionButton label="Mark spam" onClick={() => runOnSelection("Mark spam", { kind: "spam", value: true })}>
                  <ShieldAlert size={16} />
                </ActionButton>
              </>
            )}
            <ActionButton label="Mark read" onClick={() => runOnSelection("Mark read", { kind: "read", value: true })}>
              <MailOpen size={16} />
            </ActionButton>
            <ActionButton label="Mark unread" onClick={() => runOnSelection("Mark unread", { kind: "read", value: false })}>
              <Mail size={16} />
            </ActionButton>
            <ActionButton label="Star" onClick={() => runOnSelection("Star", { kind: "star", value: true })}>
              <Star size={16} />
            </ActionButton>
            <ActionButton label="Unstar" onClick={() => runOnSelection("Unstar", { kind: "star", value: false })}>
              <Star size={16} />
            </ActionButton>
            <ActionButton label="Labels" onClick={() => setLabelTargetIds([...checkedIds])}>
              <Tag size={16} />
            </ActionButton>
            <button
              className="icon-button"
              aria-label="Clear selection"
              onClick={() => setCheckedIds(new Set())}
            >
              <X size={16} />
            </button>
          </div>
        ) : null}
        {mailbox === "inbox" ? (
          <label className="search-box">
            <Search size={16} />
            <input
              ref={searchRef}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search mail"
              aria-label="Search mail"
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
        ) : null}
        <div className="thread-list" role={isThreadMailbox ? "listbox" : "list"} aria-label={MAILBOX_TITLES[mailbox]}>
          {mailbox === "drafts" ? (
            <DraftsList drafts={correspondence.drafts} onOpen={correspondence.openDraft} />
          ) : mailbox === "outbox" ? (
            <OutboxList
              outbox={correspondence.outbox}
              clock={correspondence.clock}
              onUndo={correspondence.undoSendItem}
              onRestore={correspondence.restoreFailedSend}
              onReconcile={correspondence.reconcileSend}
            />
          ) : (
            <>
          {loading ? <p className="empty">Loading inbox…</p> : null}
          {!loading && threads.length === 0 ? (
            accounts.length === 0 ? (
              <div className="connect-account-cta">
                <Mail size={28} />
                <p>Connect your Gmail account to start syncing mail.</p>
                <button type="button" onClick={() => openSettingsAt("account")}>
                  Connect Gmail
                </button>
              </div>
            ) : (
              <p className="empty">Inbox zero.</p>
            )
          ) : null}
          {threads.map((thread) => (
            <button
              key={thread.id}
              role="option"
              aria-selected={thread.id === selectedId}
              className={`thread-row ${thread.id === selectedId ? "selected" : ""}`}
              onClick={() => setSelectedId(thread.id)}
            >
              <span
                className={`row-check ${checkedIds.has(thread.id) ? "checked" : ""}`}
                aria-hidden="true"
                onClick={(event) => {
                  event.stopPropagation();
                  setCheckedIds((current) => {
                    const next = new Set(current);
                    if (next.has(thread.id)) next.delete(thread.id);
                    else next.add(thread.id);
                    return next;
                  });
                }}
              >
                {checkedIds.has(thread.id) ? <CheckSquare size={16} /> : <Square size={16} />}
              </span>
              {checkedIds.has(thread.id) ? <span className="sr-only">Selected for batch actions</span> : null}
              <span className={`unread-dot ${thread.unread ? "visible" : ""}`} />
              <span className="thread-content">
                <span className="thread-meta">
                  <span className="thread-sender">
                    {accounts.length > 1 ? (
                      <span
                        className="account-dot"
                        aria-hidden="true"
                        style={{ background: accounts.find((account) => account.email === thread.accountId)?.color }}
                      />
                    ) : null}
                    <strong>{thread.participants.join(", ")}</strong>
                  </span>
                  <time>{timeFormatter.format(new Date(thread.lastMessageAt))}</time>
                </span>
                <span className="thread-subject">{thread.subject}</span>
                <span className="thread-snippet"><HighlightedSnippet thread={thread} /></span>
              </span>
              {thread.starred ? <Star className="starred" size={15} fill="currentColor" /> : null}
            </button>
          ))}
          {hasMoreResults ? (
            <button className="load-more" onClick={() => void loadMoreResults()}>
              Load more results
            </button>
          ) : null}
            </>
          )}
        </div>
      </section>

      <section className="reader" aria-label="Conversation">
        {detail ? (
          <>
            <header className="reader-header">
              <div>
                <span className="eyebrow">
                  {detail.thread.labels.map((id) => labels.find((label) => label.id === id)?.name ?? id).join(" · ")}
                </span>
                <h2>{detail.thread.subject}</h2>
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
            <div className="reply-toolbar" aria-label="Correspondence actions">
              <ActionButton label="Reply" shortcut="r" onClick={() => executeById("draft.reply")}><Reply size={16} /></ActionButton>
              <ActionButton label="Reply all" shortcut="a" onClick={() => executeById("draft.replyAll")}><ReplyAll size={16} /></ActionButton>
              <ActionButton label="Forward" shortcut="f" onClick={() => executeById("draft.forward")}><Forward size={16} /></ActionButton>
            </div>
            <div className="message-stack">
              {detail.messages.map((message) => (
                <article className="message" key={message.id}>
                  <header>
                    <div className="avatar">{message.sender.charAt(0)}</div>
                    <div className="message-header-details">
                      <div className="message-sender-row">
                        <strong><AddressWithCopy address={message.sender} /></strong>
                        <time>{new Date(message.sentAt).toLocaleString()}</time>
                      </div>
                      <div className="message-recipients">
                        to{" "}
                        {message.recipients.map((recipient, index) => (
                          <span key={recipient}>
                            {index > 0 ? ", " : ""}
                            <AddressWithCopy address={recipient} />
                          </span>
                        ))}
                      </div>
                    </div>
                  </header>
                  <SafeMessage html={message.bodyHtml} text={message.bodyText} />
                </article>
              ))}
            </div>
          </>
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
          extraCommands={accountCommands}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {shortcutHelpOpen ? (
        <ShortcutHelp extraCommands={accountCommands} onClose={() => setShortcutHelpOpen(false)} />
      ) : null}
      {diagnosticsOpen ? (
        <Diagnostics status={syncStatus} onClose={() => setDiagnosticsOpen(false)} />
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
          onThemeChange={(next) => {
            saveTheme(next);
            setTheme(next);
          }}
          fontScale={fontScale}
          onFontScaleChange={(value) => setFontScale(saveFontScale(value))}
          fontFamily={fontFamily}
          onFontFamilyChange={(value) => setFontFamily(saveFontFamily(value))}
          authStatus={authStatus}
          onConnectAccount={async () => {
            const status = await mailClient.connectGoogle();
            setSyncStatus(status);
            setAuthStatus(await mailClient.googleAuthStatus());
            await loadThreads(query);
            setLabels(await mailClient.listLabels());
            await refreshAccounts();
          }}
          onDisconnectAccount={async () => {
            await mailClient.disconnectGoogle();
            setAuthStatus(await mailClient.googleAuthStatus());
            await refreshAccounts();
          }}
          accounts={accounts}
          onAddAccount={async () => {
            await mailClient.addAccount();
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
          }}
          onSetAccountColor={async (email, color) => {
            await mailClient.setAccountColor(email, color);
            await refreshAccounts();
          }}
          onReorderAccounts={async (emails) => {
            await mailClient.reorderAccounts(emails);
            await refreshAccounts();
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
    </main>
  );
}

function AddressWithCopy({ address }: { address: string }) {
  const [copied, setCopied] = useState(false);
  const parsed = parseAddress(address);

  const handleCopy = async (event: React.MouseEvent) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(parsed.email);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <span className="address" tabIndex={0}>
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

function parseAddress(value: string): { name: string; email: string } {
  const trimmed = value.trim();
  const openBracket = trimmed.lastIndexOf("<");
  const closeBracket = trimmed.lastIndexOf(">");

  if (openBracket > 0 && closeBracket > openBracket) {
    const email = trimmed.slice(openBracket + 1, closeBracket).trim();
    const name = trimmed
      .slice(0, openBracket)
      .trim()
      .replace(/^("|')|("|')$/g, "")
      .trim();
    if (email) return { name: name || email, email };
  }

  return { name: trimmed, email: trimmed };
}

function AccountSwitcher({
  accounts,
  activeAccountId,
  onSwitch,
  onShowAll,
}: {
  accounts: Account[];
  activeAccountId: string | null;
  onSwitch(email: string): void;
  onShowAll(): void;
}) {
  if (accounts.length <= 1) return null;

  return (
    <div className="account-rail" role="radiogroup" aria-label="Filter by account">
      <HoverTooltip label="All accounts">
        <button
          type="button"
          role="radio"
          aria-checked={activeAccountId === null}
          aria-label="All accounts"
          className={`account-icon all-accounts ${activeAccountId === null ? "active" : ""}`}
          onClick={onShowAll}
        />
      </HoverTooltip>
      {accounts.map((account) => {
        const name = account.displayName ?? account.email;
        return (
          <HoverTooltip key={account.email} label={name}>
            <button
              type="button"
              role="radio"
              aria-checked={activeAccountId === account.email}
              aria-label={name}
              className={`account-icon ${activeAccountId === account.email ? "active" : ""}`}
              style={{ background: account.color }}
              onClick={() => onSwitch(account.email)}
            >
              {name.charAt(0).toUpperCase()}
            </button>
          </HoverTooltip>
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
      <p className="shortcut-help-intro">Use Dispatch without leaving the keyboard.</p>
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

function Diagnostics({
  status,
  onClose,
}: {
  status: SyncStatus | null;
  onClose(): void;
}) {
  return (
    <Modal title="Sync diagnostics" onClose={onClose}>
      <dl className="diagnostics">
        <dt>State</dt><dd>{status?.state ?? "unknown"}</dd>
        <dt>Last successful sync</dt>
        <dd>{status?.lastSuccessfulSync ? new Date(status.lastSuccessfulSync).toLocaleString() : "Never"}</dd>
        <dt>History cursor</dt><dd>{status?.cursor ?? "Not initialized"}</dd>
        <dt>Pending mutations</dt><dd>{status?.pendingMutations ?? 0}</dd>
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
        {labels.filter((label) => label.kind === "user").map((label) => (
          <div key={label.id}>
            <label>
              <input
                type="checkbox"
                checked={checkedLabelIds.has(label.id)}
                onChange={(event) => onToggle(label.id, event.target.checked)}
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
  { id: "account", label: "Account" },
  { id: "accounts", label: "Accounts" },
  { id: "ai", label: "AI provider" },
  { id: "privacy", label: "Privacy" },
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
  authStatus,
  onConnectAccount,
  onDisconnectAccount,
  accounts,
  onAddAccount,
  onRemoveAccount,
  onReconnectAccount,
  onSetAccountColor,
  onReorderAccounts,
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
  authStatus: AuthStatus | null;
  onConnectAccount(): Promise<void>;
  onDisconnectAccount(): Promise<void>;
  accounts: Account[];
  onAddAccount(): Promise<void>;
  onRemoveAccount(email: string): Promise<void>;
  onReconnectAccount(email: string): Promise<void>;
  onSetAccountColor(email: string, color: string): Promise<void>;
  onReorderAccounts(emails: string[]): Promise<void>;
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
          {section === "account" ? (
            <AccountSettings
              status={authStatus}
              onConnect={onConnectAccount}
              onDisconnect={onDisconnectAccount}
            />
          ) : null}
          {section === "accounts" ? (
            <AccountsSettings
              accounts={accounts}
              onAdd={onAddAccount}
              onRemove={onRemoveAccount}
              onReconnect={onReconnectAccount}
              onSetColor={onSetAccountColor}
              onReorder={onReorderAccounts}
            />
          ) : null}
          {section === "ai" ? <AiProviderSettings /> : null}
          {section === "privacy" ? <PrivacySettings /> : null}
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

      <h3>Font family</h3>
      <select
        aria-label="Font family"
        value={fontFamily}
        onChange={(event) => onFontFamilyChange(event.target.value as FontFamily)}
      >
        {FONT_FAMILY_OPTIONS.map((option) => (
          <option key={option.value} value={option.value}>{option.label}</option>
        ))}
      </select>
    </section>
  );
}

function AccountSettings({
  status,
  onConnect,
  onDisconnect,
}: {
  status: AuthStatus | null;
  onConnect(): Promise<void>;
  onDisconnect(): Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const act = (operation: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    void operation()
      .catch((reason: unknown) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      )
      .finally(() => setBusy(false));
  };
  return (
    <section className="settings-section account-manager" aria-label="Account">
      {!status?.configured ? (
        <>
          <strong>Google OAuth is not configured</strong>
          <p>
            Set <code>DISPATCH_GOOGLE_CLIENT_ID</code> and{" "}
            <code>DISPATCH_GOOGLE_CLIENT_SECRET</code> from a Google Desktop
            app credential, then restart Dispatch.
          </p>
        </>
      ) : status.connected ? (
        <>
          <strong>Gmail is connected</strong>
          <p>Credentials are stored in the operating-system keychain.</p>
          <button disabled={busy} onClick={() => act(onDisconnect)}>
            Disconnect Gmail
          </button>
        </>
      ) : (
        <>
          <strong>Connect Gmail</strong>
          <p>Authorization opens in your browser and returns over a local loopback port.</p>
          <button disabled={busy} onClick={() => act(onConnect)}>
            {busy ? "Waiting for Google…" : "Continue with Google"}
          </button>
        </>
      )}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
  );
}

function AccountsSettings({
  accounts,
  onAdd,
  onRemove,
  onReconnect,
  onSetColor,
  onReorder,
}: {
  accounts: Account[];
  onAdd(): Promise<void>;
  onRemove(email: string): Promise<void>;
  onReconnect(email: string): Promise<void>;
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
    <section className="settings-section accounts-manager" aria-label="Accounts">
      <p>
        The inbox merges every connected account by default. Switch to one account,
        show all again, or jump straight to an account with <kbd>⌘1</kbd>–<kbd>⌘9</kbd>
        from the sidebar switcher or command palette.
      </p>
      {accounts.length === 0 ? (
        <p>Connect a Gmail account from the Account tab to get started.</p>
      ) : (
        <ul className="accounts-list">
          {accounts.map((account, index) => (
            <li key={account.email}>
              <span className="account-dot" aria-hidden="true" style={{ background: account.color }} />
              <span className="accounts-list-email">
                {account.email}
                {account.status === "needs_reauth" ? <em> · needs reconnect</em> : null}
              </span>
              <span className="accounts-list-actions">
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
                {account.status === "needs_reauth" ? (
                  <button
                    type="button"
                    disabled={busyEmail !== null}
                    onClick={() => act(account.email, () => onReconnect(account.email))}
                  >
                    Reconnect
                  </button>
                ) : null}
                <button
                  type="button"
                  disabled={busyEmail !== null}
                  onClick={() => act(account.email, () => onRemove(account.email))}
                >
                  Remove
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {accounts.length > 0 ? (
        <button type="button" disabled={busyEmail !== null} onClick={() => act("__add__", onAdd)}>
          {busyEmail === "__add__" ? "Waiting for Google…" : "Add another account"}
        </button>
      ) : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
    </section>
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

const AI_MODEL_PLACEHOLDERS: Record<AiProvider, string> = {
  none: "",
  openai: "gpt-4o",
  anthropic: "claude-sonnet-5",
  custom: "model name",
};

function AiProviderSettings() {
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
  };

  return (
    <section className="settings-section" aria-label="AI provider">
      <p className="settings-hint">
        Disabled by default. Dispatch only sends thread content to your chosen
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
          <label className="settings-checkbox">
            <input
              type="checkbox"
              checked={features.classify}
              onChange={(event) => updateFeature("classify", event.target.checked)}
            />
            Split Inbox classification
          </label>
        </>
      ) : null}
    </section>
  );
}

function PrivacySettings() {
  const [reporting, setReporting] = useState(crashReportingEnabled);
  const [reportCount, setReportCount] = useState(() => localCrashReports().length);
  return (
    <section className="settings-section" aria-label="Privacy">
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
            ? "Dispatch will send the sender's one-click request without opening a web page."
            : method === "mailto"
              ? "Dispatch will open a new email in your default mail handler. You will still need to send it."
              : "Dispatch will open the sender's unsubscribe page in your default browser."}
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
