import {
  Archive,
  Check,
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
  MailOpen,
  Moon,
  Sun,
  Pencil,
  RefreshCw,
  Search,
  Settings as SettingsIcon,
  Star,
  Tag,
  Trash2,
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
  commands,
  isEditableTarget,
  labelCommand,
  matchesShortcut,
  shortcutSteps,
  type Command,
  type CommandContext,
  type CommandResult,
} from "./commands";
import {
  clearLocalCrashReports,
  crashReportingEnabled,
  localCrashReports,
  setCrashReportingEnabled,
} from "./crashReporting";
import { mailClient } from "./data/client";
import { formattingShortcuts } from "./richText";
import type {
  AuthStatus,
  Label,
  SyncStatus,
  Thread,
  ThreadDetail,
  ThreadMutation,
} from "./domain";
import { InboxResizeHandle, useInboxWidth } from "./InboxResizeHandle";
import { useCorrespondence } from "./useCorrespondence";
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

type SettingsSection = "appearance" | "account" | "ai" | "privacy";

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

function useShortcutHandler(
  context: CommandContext,
  execute: (command: Command) => void,
) {
  const contextRef = useRef(context);
  contextRef.current = context;
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

      if (pendingStep.current) {
        const command = commands.find(
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

      const command = commands.find(
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

      const prefix = commands
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
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const correspondence = useCorrespondence(detail?.messages.at(-1)?.id);
  const [query, setQuery] = useState("");
  const [includeArchived, setIncludeArchived] = useState(false);
  const [hasMoreResults, setHasMoreResults] = useState(false);
  const [loading, setLoading] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [labelsOpen, setLabelsOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("appearance");
  const [labels, setLabels] = useState<Label[]>([]);
  const [authStatus, setAuthStatus] = useState<AuthStatus | null>(null);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [notice, setNotice] = useNotice();
  const lastUndo = useRef<{ command: Command; result: CommandResult } | null>(null);
  const [canUndoAction, setCanUndoAction] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

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

  const loadThreads = useCallback(async (search: string) => {
    const trimmed = search.trim();
    const next = trimmed
      ? await mailClient.searchThreads({
          query: trimmed,
          limit: SEARCH_PAGE_SIZE,
          includeArchived,
        })
      : await mailClient.listThreads();
    setThreads(next);
    setHasMoreResults(trimmed ? next.length === SEARCH_PAGE_SIZE : false);
    setSelectedId((current) =>
      current && next.some((thread) => thread.id === current)
        ? current
        : (next[0]?.id ?? null),
    );
  }, [includeArchived]);

  const loadMoreResults = useCallback(async () => {
    const trimmed = query.trim();
    if (!trimmed) return;
    const next = await mailClient.searchThreads({
      query: trimmed,
      limit: SEARCH_PAGE_SIZE,
      offset: threads.length,
      includeArchived,
    });
    setThreads((current) => [...current, ...next]);
    setHasMoreResults(next.length === SEARCH_PAGE_SIZE);
  }, [query, threads.length, includeArchived]);

  useEffect(() => {
    if (correspondence.sentCount > 0) void loadThreads(query);
  }, [correspondence.sentCount, loadThreads]);

  useEffect(() => {
    Promise.all([loadThreads(""), mailClient.syncStatus(), mailClient.googleAuthStatus()])
      .then(([, status, auth]) => {
        setSyncStatus(status);
        setAuthStatus(auth);
        void mailClient.listLabels().then(setLabels).catch(() => setLabels([]));
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

  useEffect(() => {
    let timer = 0;
    const flushIfInactive = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        if (document.visibilityState === "visible" && document.hasFocus()) return;
        void mailClient.flushPending().then(setSyncStatus).catch(() => {});
      }, 150);
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") flushIfInactive();
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("blur", flushIfInactive);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("blur", flushIfInactive);
    };
  }, []);

  useEffect(() => {
    const timeout = window.setTimeout(() => void loadThreads(query), 180);
    return () => window.clearTimeout(timeout);
  }, [query, loadThreads]);

  const mutate = useCallback(async (mutation: ThreadMutation): Promise<CommandResult> => {
    const previous = threads.find((thread) => thread.id === mutation.threadId);
    if (!previous) return {};

    const applyLocal = (thread: Thread): Thread => {
      if (thread.id !== mutation.threadId) return thread;
      if (mutation.kind === "archive") return { ...thread, archived: mutation.value };
      if (mutation.kind === "read") return { ...thread, unread: !mutation.value };
      if (mutation.kind === "star") return { ...thread, starred: mutation.value };
      const threadLabels = new Set(thread.labels);
      if (mutation.value) threadLabels.add(mutation.labelId);
      else threadLabels.delete(mutation.labelId);
      return { ...thread, labels: [...threadLabels] };
    };

    setThreads((current) =>
      current.map(applyLocal).filter((thread) => !thread.archived),
    );
    if (mutation.kind === "archive") {
      if (mutation.value && selectedId === mutation.threadId) {
        const currentIndex = threads.findIndex((thread) => thread.id === mutation.threadId);
        const remaining = threads.filter((thread) => thread.id !== mutation.threadId);
        const nextIndex = Math.min(currentIndex, remaining.length - 1);
        setSelectedId(remaining[nextIndex]?.id ?? null);
      }
    }

    try {
      await mailClient.mutateThread(mutation);
      if (mutation.kind !== "archive") await loadThreads(query);
      if (document.visibilityState !== "visible" || !document.hasFocus()) {
        void mailClient.flushPending().then(setSyncStatus).catch(() => {});
      }
      const undoMutation = { ...mutation, value: !mutation.value } as ThreadMutation;
      return {
        message: mutation.kind === "archive"
          ? "Conversation archived"
          : mutation.kind === "label"
            ? `${labels.find((label) => label.id === mutation.labelId)?.name ?? "Label"} ${mutation.value ? "added" : "removed"}`
            : undefined,
        undoAction: async () => {
          setThreads((current) => [
            ...current.filter((thread) => thread.id !== previous.id),
            previous,
          ].sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt)));
          if (mutation.kind === "archive") setSelectedId(previous.id);
          await mailClient.mutateThread(undoMutation);
          await loadThreads(query);
        },
      };
    } catch {
      setThreads((current) => [
        ...current.filter((thread) => thread.id !== previous.id),
        previous,
      ].sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt)));
      setNotice({ message: "Change could not be saved" });
      return {};
    }
  }, [labels, loadThreads, query, threads, selectedId, setNotice]);

  const selected = threads.find((thread) => thread.id === selectedId) ?? null;
  const selectedIndex = threads.findIndex((thread) => thread.id === selectedId);

  const openSettingsAt = useCallback((section: SettingsSection) => {
    setSettingsSection(section);
    setSettingsOpen(true);
  }, []);

  const context = useMemo<CommandContext>(() => ({
    ...correspondence.context,
    selectedId,
    openInbox: () => {
      correspondence.context.openInbox();
      setQuery("");
      void loadThreads("");
    },
    selectNext: () => {
      const next = Math.min(selectedIndex + 1, threads.length - 1);
      setSelectedId(threads[next]?.id ?? null);
    },
    selectPrevious: () => {
      const next = Math.max(selectedIndex - 1, 0);
      setSelectedId(threads[next]?.id ?? null);
    },
    archiveSelected: () => {
      if (selected) return mutate({ kind: "archive", threadId: selected.id, value: true });
      return Promise.resolve({});
    },
    setLabelSelected: (labelId, value) => {
      if (selected) return mutate({ kind: "label", threadId: selected.id, labelId, value });
      return Promise.resolve({});
    },
    toggleReadSelected: () => {
      if (selected) void mutate({ kind: "read", threadId: selected.id, value: selected.unread });
    },
    toggleStarSelected: () => {
      if (selected) void mutate({ kind: "star", threadId: selected.id, value: !selected.starred });
    },
    focusSearch: () => searchRef.current?.focus(),
    refresh: () => {
      setSyncStatus((current) => current ? { ...current, state: "syncing" } : current);
      void mailClient.sync().then((status) => {
        setSyncStatus(status);
        void loadThreads(query);
      });
    },
    openDiagnostics: () => setDiagnosticsOpen(true),
    openLabels: () => setLabelsOpen(true),
    openPalette: () => setPaletteOpen(true),
    openShortcutHelp: () => setShortcutHelpOpen(true),
    openSettings: () => openSettingsAt("appearance"),
    increaseFontSize: () => adjustFontScale(1),
    decreaseFontSize: () => adjustFontScale(-1),
    canUndoAction,
    undoLastAction: () => { void undoLastAction(); },
  }), [adjustFontScale, canUndoAction, loadThreads, mutate, openSettingsAt, query, selected, selectedId, selectedIndex, threads, correspondence.context, undoLastAction]);

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
  useShortcutHandler(context, executeCommand);

  return (
    <main className="app-shell" style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
      <nav className="sidebar" aria-label="Mailboxes">
        <button className="brand" aria-label="Account" onClick={() => openSettingsAt("account")}>D</button>
        <HoverTooltip label="Inbox" shortcut="G I">
          <button
            className="nav-button active"
            aria-label="Inbox (g then i)"
            onClick={() => executeById("mailbox.inbox")}
          >
            <Inbox size={19} />
          </button>
        </HoverTooltip>
        <button className="nav-button" aria-label="New message (c)" title="New message (c)" onClick={() => executeById("draft.new")}><Pencil size={19} /></button>
        <HoverTooltip label="Drafts" shortcut="G D">
          <button className="nav-button" aria-label={`Drafts (${correspondence.draftCount}) (g then d)`} onClick={() => executeById("drafts.open")}><FileText size={19} /></button>
        </HoverTooltip>
        <HoverTooltip label="Outbox">
          <button className="nav-button" aria-label={`Outbox (${correspondence.outboxCount})`} onClick={() => executeById("outbox.open")}><Send size={19} /></button>
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
          <div>
            <span className="eyebrow">Inbox</span>
            <h1>{threads.length} conversations</h1>
          </div>
          <button
            className="icon-button"
            aria-label="Refresh mail"
            onClick={() => executeById("mail.refresh")}
          >
            <RefreshCw size={17} className={syncStatus?.state === "syncing" ? "spin" : ""} />
          </button>
        </header>
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
              aria-label="Include archived mail in search"
              title="Include archived mail in search"
              onClick={() => setIncludeArchived((current) => !current)}
            >
              <Archive size={14} />
            </button>
          ) : null}
          <kbd>/</kbd>
        </label>
        <div className="thread-list" role="listbox" aria-label="Conversations">
          {loading ? <p className="empty">Loading inbox…</p> : null}
          {!loading && threads.length === 0 ? <p className="empty">Inbox zero.</p> : null}
          {threads.map((thread) => (
            <button
              key={thread.id}
              role="option"
              aria-selected={thread.id === selectedId}
              className={`thread-row ${thread.id === selectedId ? "selected" : ""}`}
              onClick={() => setSelectedId(thread.id)}
            >
              <span className={`unread-dot ${thread.unread ? "visible" : ""}`} />
              <span className="thread-content">
                <span className="thread-meta">
                  <strong>{thread.participants.join(", ")}</strong>
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
        </div>
      </section>

      <section className="reader" aria-label="Conversation">
        {detail ? (
          <>
            <header className="reader-header">
              <div>
                <span className="eyebrow">{detail.thread.labels.join(" · ")}</span>
                <h2>{detail.thread.subject}</h2>
              </div>
              <div className="reader-actions">
                <ActionButton
                  label={selected?.starred ? "Unstar" : "Star"}
                  shortcut="s"
                  onClick={() => executeById("thread.star")}
                >
                  <Star size={17} fill={selected?.starred ? "currentColor" : "none"} />
                </ActionButton>
                <ActionButton
                  label={selected?.unread ? "Mark read" : "Mark unread"}
                  shortcut="u"
                  onClick={() => executeById("thread.read")}
                >
                  {selected?.unread ? <MailOpen size={17} /> : <Mail size={17} />}
                </ActionButton>
                <HoverTooltip label="Manage Labels" shortcut="L" placement="bottom">
                  <ActionButton label="Labels" shortcut="l" onClick={() => executeById("labels.open")}>
                    <Tag size={17} />
                  </ActionButton>
                </HoverTooltip>
                <ActionButton label="Archive" shortcut="e" onClick={() => executeById("thread.archive")}>
                  <Archive size={17} />
                </ActionButton>
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
                    <div>
                      <strong><AddressWithCopy address={message.sender} /></strong>
                      <span>
                        to{" "}
                        {message.recipients.map((recipient, index) => (
                          <span key={recipient}>
                            {index > 0 ? ", " : ""}
                            <AddressWithCopy address={recipient} />
                          </span>
                        ))}
                      </span>
                    </div>
                    <time>{new Date(message.sentAt).toLocaleString()}</time>
                  </header>
                  <SafeMessage html={message.bodyHtml} text={message.bodyText} />
                </article>
              ))}
            </div>
          </>
        ) : (
          <div className="reader-empty"><Mail size={28} /><p>Select a conversation</p></div>
        )}
      </section>

      {correspondence.overlay}
      {paletteOpen ? (
        <CommandPalette context={context} execute={executeCommand} onClose={() => setPaletteOpen(false)} />
      ) : null}
      {shortcutHelpOpen ? (
        <ShortcutHelp onClose={() => setShortcutHelpOpen(false)} />
      ) : null}
      {diagnosticsOpen ? (
        <Diagnostics status={syncStatus} onClose={() => setDiagnosticsOpen(false)} />
      ) : null}
      {labelsOpen && selected ? (
        <LabelManager
          labels={labels}
          thread={selected}
          onClose={() => setLabelsOpen(false)}
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
          }}
          onDisconnectAccount={async () => {
            await mailClient.disconnectGoogle();
            setAuthStatus(await mailClient.googleAuthStatus());
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

  const handleCopy = async (event: React.MouseEvent) => {
    event.stopPropagation();
    await navigator.clipboard.writeText(address);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  return (
    <span className="address">
      {address}
      <button
        type="button"
        className="address-copy"
        aria-label={copied ? "Copied" : `Copy ${address}`}
        onClick={handleCopy}
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
    </span>
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
  shortcut: string;
}) {
  return (
    <button className="action-button" aria-label={`${label} (${shortcut})`} onClick={onClick}>
      {children}<span>{label}</span><kbd>{shortcut}</kbd>
    </button>
  );
}

function CommandPalette({
  context,
  execute,
  onClose,
}: {
  context: CommandContext;
  execute(command: Command): void;
  onClose(): void;
}) {
  const [filter, setFilter] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => inputRef.current?.focus(), []);
  const visible = commands.filter((command) =>
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

function ShortcutHelp({ onClose }: { onClose(): void }) {
  const shortcutCommands = [
    ...commands.filter((command) => command.keys.length > 0),
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
  thread,
  onClose,
  onCreate,
  onDelete,
  onRename,
  onToggle,
}: {
  labels: Label[];
  thread: Thread;
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
        {labels.map((label) => (
          <div key={label.id}>
            <label>
              <input
                type="checkbox"
                checked={thread.labels.includes(label.id)}
                onChange={(event) => onToggle(label.id, event.target.checked)}
              />
              <span className="label-color" style={{ background: label.color ?? "#64646d" }} />
              {label.name}
            </label>
            {label.kind === "user" ? (
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
            ) : null}
          </div>
        ))}
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
