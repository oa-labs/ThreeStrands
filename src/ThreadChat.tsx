import { FileText, MessageSquareText, RotateCcw, Sparkles, X } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import {
  MAX_CHAT_ATTACHMENTS,
  activeMention,
  applyMention,
  attachmentKey,
  matchAttachments,
  type ChatAttachmentOption,
  type Mention,
} from "./chatAttachments";
import type { ChatAttachmentSource, ChatAvailability, ChatSource } from "./domain";

export type ChatEntry =
  | {
    id: string;
    role: "user";
    content: string;
    searchMailbox: boolean;
    /** Attachments first shared with this question. */
    attachments: ChatAttachmentOption[];
  }
  | {
    id: string;
    role: "assistant";
    content: string;
    replyDraft: string | null;
    addedSuggestions: number;
    hiddenSuggestions: number;
    sources: ChatSource[];
    searched: ChatSource[];
    /** Attachments whose text the answer could see. */
    attachments: ChatAttachmentSource[];
    /** A range to show open times for, taken from the user's calendar. */
    availability: ChatAvailability | null;
  };

/** Attachments the chat's questions shared so far, each once, in order. */
export function sharedChatAttachments(entries: ChatEntry[]): ChatAttachmentOption[] {
  const shared = new Map<string, ChatAttachmentOption>();
  for (const entry of entries) {
    if (entry.role !== "user") continue;
    for (const attachment of entry.attachments) {
      if (!shared.has(attachmentKey(attachment))) shared.set(attachmentKey(attachment), attachment);
    }
  }
  return [...shared.values()];
}

export const QUICK_QUESTIONS = [
  "Draft a reply",
  "What do they need from me?",
  "What’s still open here?",
] as const;

/**
 * Questions about the open conversation. In read mode it is a single
 * prompt button, so single-key shortcuts stay available; clicking it or
 * pressing the chat shortcut (which bumps `focusRequest`) turns it into a
 * text box marked as an entry surface. Escape returns to read mode and to
 * whatever had focus before, keeping any unsent text.
 *
 * Typing @ lists the conversation's readable attachments; choosing one
 * shares its text with the provider for this question and the rest of the
 * chat. Nothing is shared unless the user picks it.
 */
export function ThreadChat({
  enabled,
  available,
  entries,
  pending,
  error,
  focusRequest,
  attachments = [],
  sharedAttachments = [],
  onAsk,
  onRetry,
  onUseReply,
  onOpenThread,
  onShowSuggestions,
  onOpenSettings,
  renderAvailability,
}: {
  enabled: boolean;
  available: boolean;
  entries: ChatEntry[];
  pending: boolean;
  error: string | null;
  focusRequest: number;
  /** Readable attachments in the conversation, offered after @. */
  attachments?: ChatAttachmentOption[];
  /** Attachments earlier questions in this chat already shared. */
  sharedAttachments?: ChatAttachmentOption[];
  onAsk(question: string, searchMailbox: boolean, attachments: ChatAttachmentOption[]): void;
  onRetry(): void;
  onUseReply(text: string): void;
  onOpenThread(threadId: string): void;
  onShowSuggestions(): void;
  onOpenSettings(): void;
  /** Shows open times for an answer that asked for them. */
  renderAvailability?(availability: ChatAvailability): ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [searchMailbox, setSearchMailbox] = useState(false);
  const [selected, setSelected] = useState<ChatAttachmentOption[]>([]);
  const [mention, setMention] = useState<Mention | null>(null);
  const [dismissedMention, setDismissedMention] = useState<number | null>(null);
  const [highlighted, setHighlighted] = useState(0);
  const pendingCaret = useRef<number | null>(null);
  // React can report a selection change for a keystroke after the handler
  // already replaced the text; such a report describes the old text.
  const latestDraft = useRef("");
  const menuId = useId();
  const input = useRef<HTMLTextAreaElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const log = useRef<HTMLDivElement>(null);

  const activate = () => {
    if (!available) return;
    const active = document.activeElement;
    returnFocus.current = active instanceof HTMLElement && active !== document.body && !active.closest(".thread-chat") ? active : null;
    setOpen(true);
  };

  // Only a request made while this conversation is shown activates the box;
  // one made for an earlier conversation must not reopen it on mount.
  const handledRequest = useRef(focusRequest);
  useEffect(() => {
    if (focusRequest === handledRequest.current) return;
    handledRequest.current = focusRequest;
    activate();
    // Only a new request should activate; availability changes should not.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusRequest]);

  useEffect(() => {
    if (open) input.current?.focus();
  }, [open, focusRequest]);

  useEffect(() => {
    log.current?.scrollTo?.({ top: log.current.scrollHeight });
  }, [entries.length, pending]);

  const leave = () => {
    setOpen(false);
    const target = returnFocus.current;
    returnFocus.current = null;
    if (target?.isConnected) target.focus({ preventScroll: true });
    else input.current?.blur();
  };

  useLayoutEffect(() => {
    if (pendingCaret.current === null || !input.current) return;
    input.current.setSelectionRange(pendingCaret.current, pendingCaret.current);
    pendingCaret.current = null;
  }, [draft]);

  const sharedKeys = new Set([...sharedAttachments, ...selected].map(attachmentKey));
  const atLimit = sharedKeys.size >= MAX_CHAT_ATTACHMENTS;
  const offered = attachments.filter((attachment) => !sharedKeys.has(attachmentKey(attachment)));
  const matches = mention && mention.start !== dismissedMention ? matchAttachments(offered, mention.query) : [];
  const menuOpen = mention !== null && mention.start !== dismissedMention && (matches.length > 0 || (atLimit && offered.length > 0));
  const active = matches.length > 0 ? Math.min(highlighted, matches.length - 1) : -1;

  const updateDraft = (text: string) => {
    latestDraft.current = text;
    setDraft(text);
  };

  const trackMention = (text: string, caret: number | null) => {
    if (text !== latestDraft.current) return;
    const next = caret === null ? null : activeMention(text, caret);
    if (next?.start !== mention?.start || next?.query !== mention?.query) setHighlighted(0);
    setMention((current) => current?.start === next?.start && current?.query === next?.query ? current : next);
    setDismissedMention((dismissed) => next?.start === dismissed ? dismissed : null);
  };

  const choose = (option: ChatAttachmentOption) => {
    if (!mention || atLimit) return;
    const caret = input.current?.selectionStart ?? draft.length;
    const next = applyMention(draft, mention, caret, option.filename);
    pendingCaret.current = next.caret;
    updateDraft(next.text);
    setSelected((current) => [...current, option]);
    setMention(null);
    input.current?.focus();
  };

  const ask = (question: string) => {
    const text = question.trim();
    if (!text || pending) return;
    onAsk(text, searchMailbox, selected);
    updateDraft("");
    setSearchMailbox(false);
    setSelected([]);
    setMention(null);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (menuOpen && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setDismissedMention(mention!.start);
    } else if (menuOpen && matches.length > 0 && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setHighlighted((active + step + matches.length) % matches.length);
    } else if (menuOpen && matches.length > 0 && !atLimit && (event.key === "Enter" || event.key === "Tab") && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      choose(matches[active]);
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      leave();
    } else if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      ask(draft);
    }
  };

  return <section className="context-section thread-chat" aria-label="Ask about this conversation">
    {entries.length > 0 ? <div className="thread-chat-log" role="log" aria-label="Conversation with AI" ref={log}>
      {entries.map((entry) => entry.role === "user" ? (
        <p key={entry.id} className="thread-chat-question">
          {entry.content}
          {entry.attachments.length > 0 ? <small>Shared {entry.attachments.map((attachment) => attachment.filename).join(", ")}</small> : null}
          {entry.searchMailbox ? <small>Searched all mail</small> : null}
        </p>
      ) : (
        <div key={entry.id} className="thread-chat-answer">
          <p>{entry.content}</p>
          {entry.availability && renderAvailability ? renderAvailability(entry.availability) : null}
          {entry.replyDraft ? <div className="thread-chat-draft">
            <pre>{entry.replyDraft}</pre>
            <button type="button" className="btn btn-sm" onClick={() => onUseReply(entry.replyDraft!)}>Use as Reply</button>
          </div> : null}
          {entry.addedSuggestions > 0 ? <button type="button" className="btn-link context-link-button" onClick={onShowSuggestions}>
            Added {entry.addedSuggestions === 1 ? "1 suggestion" : `${entry.addedSuggestions} suggestions`} to review
          </button> : null}
          {entry.hiddenSuggestions > 0 ? <small>{entry.hiddenSuggestions === 1 ? "1 suggestion" : `${entry.hiddenSuggestions} suggestions`} couldn&rsquo;t be matched to the email, so {entry.hiddenSuggestions === 1 ? "it was" : "they were"} hidden.</small> : null}
          {entry.sources.length > 0 ? <nav className="thread-chat-sources" aria-label="Sources">
            {entry.sources.map((source) => <button type="button" className="btn-link" key={source.threadId} onClick={() => onOpenThread(source.threadId)}>{source.subject || "(no subject)"}</button>)}
          </nav> : null}
          {entry.attachments.filter((attachment) => attachment.truncated).map((attachment) => <small key={attachmentKey(attachment)}>
            {attachment.filename} is long, so only its first part was shared.
          </small>)}
          {entry.searched.length > 0 ? <details className="thread-chat-searched">
            <summary>Shared {entry.searched.length === 1 ? "1 other email" : `${entry.searched.length} other emails`} with AI</summary>
            {entry.searched.map((source) => <button type="button" className="btn-link" key={source.threadId} onClick={() => onOpenThread(source.threadId)}>{source.subject || "(no subject)"}</button>)}
          </details> : null}
        </div>
      ))}
      {pending ? <p className="context-status" role="status">Thinking…</p> : null}
    </div> : pending ? <p className="context-status" role="status">Thinking…</p> : null}
    {error ? <div className="action-analysis-error" role="alert">
      <p>{error}</p>
      <div className="action-analysis-error-actions"><button type="button" className="btn btn-sm" onClick={onRetry}><RotateCcw size={13} /> Try Again</button></div>
    </div> : null}
    {!enabled || !available ? <>
      <p className="context-status">{!enabled ? "Turn on Thread Chat in AI settings to ask questions about this conversation." : "Set up an AI provider and API key in AI settings to ask questions."}</p>
      <button type="button" className="btn-link context-link-button" onClick={onOpenSettings}>AI Settings</button>
    </> : open ? (
      <form
        className="thread-chat-form"
        data-shortcut-scope="modal"
        onSubmit={(event) => { event.preventDefault(); ask(draft); }}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null) && !draft.trim()) setOpen(false);
        }}
      >
        <textarea
          ref={input}
          aria-label="Ask about this conversation"
          aria-describedby="thread-chat-hint"
          aria-autocomplete={attachments.length > 0 ? "list" : undefined}
          aria-expanded={attachments.length > 0 ? menuOpen : undefined}
          aria-controls={menuOpen ? menuId : undefined}
          aria-activedescendant={menuOpen && active >= 0 ? `${menuId}-${active}` : undefined}
          rows={2}
          maxLength={2000}
          value={draft}
          placeholder={attachments.length > 0 ? "Ask about this conversation… Type @ to add an attachment" : "Ask about this conversation…"}
          onChange={(event) => {
            updateDraft(event.target.value);
            trackMention(event.target.value, event.target.selectionStart);
          }}
          onSelect={(event) => trackMention(event.currentTarget.value, event.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
        />
        <p id="thread-chat-hint" className="sr-only">
          Enter to ask, Shift+Enter for a new line, Escape to return to shortcuts.
          {attachments.length > 0 ? " Type @ to share an attachment with AI." : null}
        </p>
        {menuOpen ? <ul className="thread-chat-mentions" id={menuId} role="listbox" aria-label="Attachments">
          {atLimit ? <li className="thread-chat-mentions-note" role="presentation">You can share up to {MAX_CHAT_ATTACHMENTS} attachments in one chat.</li> : null}
          {atLimit ? null : matches.map((option, index) => <li
            key={attachmentKey(option)}
            id={`${menuId}-${index}`}
            role="option"
            aria-selected={index === active}
            onMouseDown={(event) => event.preventDefault()}
            onMouseEnter={() => setHighlighted(index)}
            onClick={() => choose(option)}
          >
            <FileText size={13} aria-hidden="true" />
            <span>{option.filename}</span>
            <small>{option.sender}</small>
          </li>)}
        </ul> : null}
        {sharedAttachments.length + selected.length > 0 ? <ul className="thread-chat-attachments" aria-label="Attachments shared with AI">
          {sharedAttachments.map((attachment) => <li key={attachmentKey(attachment)} title="Shared earlier in this chat">
            <FileText size={12} aria-hidden="true" /><span>{attachment.filename}</span>
          </li>)}
          {selected.map((attachment) => <li key={attachmentKey(attachment)}>
            <FileText size={12} aria-hidden="true" /><span>{attachment.filename}</span>
            <button
              type="button"
                className="btn-icon btn-icon-sm"
              aria-label={`Don’t share ${attachment.filename}`}
              onClick={() => setSelected((current) => current.filter((candidate) => attachmentKey(candidate) !== attachmentKey(attachment)))}
            ><X size={12} aria-hidden="true" /></button>
          </li>)}
        </ul> : null}
        {!draft.trim() && entries.length === 0 ? <div className="thread-chat-quick" aria-label="Suggested questions" role="group">
          {QUICK_QUESTIONS.map((question) => <button key={question} type="button" className="btn btn-sm btn-wrap" disabled={pending} onClick={() => ask(question)}>{question}</button>)}
        </div> : null}
        <div className="thread-chat-controls">
          <label title="Also search your other mail for this question only">
            <input type="checkbox" checked={searchMailbox} onChange={(event) => setSearchMailbox(event.target.checked)} />
            Search all mail
          </label>
          <button type="submit" className="btn btn-sm btn-primary" disabled={pending || !draft.trim()}><Sparkles size={13} /> Ask</button>
        </div>
      </form>
    ) : (
      <button type="button" className="thread-chat-prompt" onClick={activate} aria-keyshortcuts="q">
        <MessageSquareText size={14} aria-hidden="true" />
        <span>{draft.trim() ? draft : "Ask about this conversation…"}</span>
        <kbd aria-hidden="true">q</kbd>
      </button>
    )}
  </section>;
}
