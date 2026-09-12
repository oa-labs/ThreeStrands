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
  Mail,
  MailOpen,
  Moon,
  Sun,
  Pencil,
  RefreshCw,
  Search,
  Star,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import {
  type RefObject,
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
  matchesShortcut,
  shortcutSteps,
  type CommandContext,
} from "./commands";
import {
  clearLocalCrashReports,
  crashReportingEnabled,
  localCrashReports,
  setCrashReportingEnabled,
} from "./crashReporting";
import { mailClient } from "./data/client";
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

import { applyTheme, readTheme, saveTheme } from "./theme";

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

function useShortcutHandler(
  context: CommandContext,
  openPalette: () => void,
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
        openPalette();
        return;
      }
      const sendShortcut = event.target instanceof HTMLElement && Boolean(event.target.closest(".composer")) && currentContext.composerActive && (event.metaKey || event.ctrlKey) && event.key === "Enter";
      const dialog = document.querySelector('[role="dialog"]');
      const allowsMailboxNavigation = dialog?.classList.contains("correspondence-list");
      if (!sendShortcut && (isEditableTarget(event.target) || (dialog && !allowsMailboxNavigation))) {
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
          command.run(currentContext);
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
        command.run(currentContext);
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
  }, [openPalette]);
}

export function App() {
  const inboxSize = useInboxWidth();
  const [theme, setTheme] = useState(readTheme);
  useEffect(() => applyTheme(theme), [theme]);
  const toggleTheme = () => {
    const next = theme === "dark" ? "light" : "dark";
    saveTheme(next);
    setTheme(next);
  };
  const [threads, setThreads] = useState<Thread[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ThreadDetail | null>(null);
  const correspondence = useCorrespondence(detail?.messages.at(-1)?.id);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [diagnosticsOpen, setDiagnosticsOpen] = useState(false);
  const [labelsOpen, setLabelsOpen] = useState(false);
  const [accountOpen, setAccountOpen] = useState(false);
  const [labels, setLabels] = useState<Label[]>([]);
  const [authStatus, setAuthStatus] = useState<AuthStatus | null>(null);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [notice, setNotice] = useNotice();
  const searchRef = useRef<HTMLInputElement>(null);

  const loadThreads = useCallback(async (search: string) => {
    const next = search.trim()
      ? await mailClient.searchThreads({ query: search })
      : await mailClient.listThreads();
    setThreads(next);
    setSelectedId((current) =>
      current && next.some((thread) => thread.id === current)
        ? current
        : (next[0]?.id ?? null),
    );
  }, []);

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

  const mutate = useCallback(async (mutation: ThreadMutation) => {
    const previous = threads.find((thread) => thread.id === mutation.threadId);
    if (!previous) return;

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
      setNotice({
        message: "Conversation archived",
        undo: () => {
          void mailClient
            .mutateThread({ ...mutation, value: false })
            .then(() => loadThreads(query));
          setNotice(null);
        },
      });
    }

    try {
      await mailClient.mutateThread(mutation);
      if (mutation.kind !== "archive") await loadThreads(query);
      if (document.visibilityState !== "visible" || !document.hasFocus()) {
        void mailClient.flushPending().then(setSyncStatus).catch(() => {});
      }
    } catch {
      setThreads((current) => [
        ...current.filter((thread) => thread.id !== previous.id),
        previous,
      ].sort((a, b) => b.lastMessageAt.localeCompare(a.lastMessageAt)));
      setNotice({ message: "Change could not be saved" });
    }
  }, [loadThreads, query, threads, selectedId, setNotice]);

  const selected = threads.find((thread) => thread.id === selectedId) ?? null;
  const selectedIndex = threads.findIndex((thread) => thread.id === selectedId);

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
      if (selected) void mutate({ kind: "archive", threadId: selected.id, value: true });
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
  }), [loadThreads, mutate, query, selected, selectedId, selectedIndex, threads, correspondence.context]);

  const openPalette = useCallback(() => setPaletteOpen(true), []);
  useShortcutHandler(context, openPalette);

  return (
    <main className="app-shell" style={{ "--inbox-width": `${inboxSize.width}px` } as CSSProperties}>
      <nav className="sidebar" aria-label="Mailboxes">
        <button className="brand" aria-label="Account" onClick={() => setAccountOpen(true)}>D</button>
        <HoverTooltip label="Inbox" shortcut="G I">
          <button
            className="nav-button active"
            aria-label="Inbox (g then i)"
            onClick={context.openInbox}
          >
            <Inbox size={19} />
          </button>
        </HoverTooltip>
        <button className="nav-button" aria-label="New message (c)" title="New message (c)" onClick={context.compose}><Pencil size={19} /></button>
        <HoverTooltip label="Drafts" shortcut="G D">
          <button className="nav-button" aria-label={`Drafts (${correspondence.draftCount}) (g then d)`} onClick={context.openDrafts}><FileText size={19} /></button>
        </HoverTooltip>
        <HoverTooltip label="Outbox">
          <button className="nav-button" aria-label={`Outbox (${correspondence.outboxCount})`} onClick={context.openOutbox}><Send size={19} /></button>
        </HoverTooltip>
        <div className="sidebar-spacer" />
        <button
          className="nav-button"
          aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
          title={`Switch to ${theme === "dark" ? "light" : "dark"} mode`}
          onClick={toggleTheme}
        >
          {theme === "dark" ? <Sun size={19} /> : <Moon size={19} />}
        </button>
        <button
          className="nav-button"
          aria-label="Command palette"
          onClick={() => setPaletteOpen(true)}
        >
          <CommandIcon size={19} />
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
            onClick={context.refresh}
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
                <span className="thread-snippet">{thread.snippet}</span>
              </span>
              {thread.starred ? <Star className="starred" size={15} fill="currentColor" /> : null}
            </button>
          ))}
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
                  onClick={context.toggleStarSelected}
                >
                  <Star size={17} fill={selected?.starred ? "currentColor" : "none"} />
                </ActionButton>
                <ActionButton
                  label={selected?.unread ? "Mark read" : "Mark unread"}
                  shortcut="u"
                  onClick={context.toggleReadSelected}
                >
                  {selected?.unread ? <MailOpen size={17} /> : <Mail size={17} />}
                </ActionButton>
                <HoverTooltip label="Manage Labels" shortcut="L" placement="bottom">
                  <ActionButton label="Labels" shortcut="l" onClick={context.openLabels}>
                    <Tag size={17} />
                  </ActionButton>
                </HoverTooltip>
                <ActionButton label="Archive" shortcut="e" onClick={context.archiveSelected}>
                  <Archive size={17} />
                </ActionButton>
              </div>
            </header>
            <div className="reply-toolbar" aria-label="Correspondence actions">
              <ActionButton label="Reply" shortcut="r" onClick={context.reply}><Reply size={16} /></ActionButton>
              <ActionButton label="Reply all" shortcut="a" onClick={context.replyAll}><ReplyAll size={16} /></ActionButton>
              <ActionButton label="Forward" shortcut="f" onClick={context.forward}><Forward size={16} /></ActionButton>
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
        <CommandPalette context={context} onClose={() => setPaletteOpen(false)} />
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
          onToggle={(labelId, value) =>
            mutate({ kind: "label", threadId: selected.id, labelId, value })
          }
        />
      ) : null}
      {accountOpen ? (
        <AccountManager
          status={authStatus}
          onClose={() => setAccountOpen(false)}
          onConnect={async () => {
            const status = await mailClient.connectGoogle();
            setSyncStatus(status);
            setAuthStatus(await mailClient.googleAuthStatus());
            await loadThreads(query);
            setLabels(await mailClient.listLabels());
          }}
          onDisconnect={async () => {
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
  onClose,
}: {
  context: CommandContext;
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
              command.run(context);
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

function Diagnostics({
  status,
  onClose,
}: {
  status: SyncStatus | null;
  onClose(): void;
}) {
  const [reporting, setReporting] = useState(crashReportingEnabled);
  const [reportCount, setReportCount] = useState(() => localCrashReports().length);
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
      <div className="reporting-settings">
        <label>
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
        <span>
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
      </div>
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

function AccountManager({
  status,
  onClose,
  onConnect,
  onDisconnect,
}: {
  status: AuthStatus | null;
  onClose(): void;
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
    <Modal title="Google account" onClose={onClose}>
      <div className="account-manager">
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
      </div>
    </Modal>
  );
}

function Modal({
  children,
  onClose,
  title,
}: {
  children: React.ReactNode;
  onClose(): void;
  title: string;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [onClose]);
  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <div
        ref={panelRef as RefObject<HTMLDivElement>}
        className="modal"
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
