import { MessageSquareText, RotateCcw, Sparkles } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ChatSource } from "./domain";

export type ChatEntry =
  | { id: string; role: "user"; content: string; searchMailbox: boolean }
  | {
    id: string;
    role: "assistant";
    content: string;
    replyDraft: string | null;
    addedSuggestions: number;
    hiddenSuggestions: number;
    sources: ChatSource[];
    searched: ChatSource[];
  };

export const QUICK_QUESTIONS = [
  "What do they need from me?",
  "What’s still open here?",
  "Draft a reply",
] as const;

/**
 * Questions about the open conversation. In read mode it is a single
 * prompt button, so single-key shortcuts stay available; clicking it or
 * pressing the chat shortcut (which bumps `focusRequest`) turns it into a
 * text box marked as an entry surface. Escape returns to read mode and to
 * whatever had focus before, keeping any unsent text.
 */
export function ThreadChat({
  enabled,
  available,
  entries,
  pending,
  error,
  focusRequest,
  onAsk,
  onRetry,
  onUseReply,
  onOpenThread,
  onShowSuggestions,
  onOpenSettings,
}: {
  enabled: boolean;
  available: boolean;
  entries: ChatEntry[];
  pending: boolean;
  error: string | null;
  focusRequest: number;
  onAsk(question: string, searchMailbox: boolean): void;
  onRetry(): void;
  onUseReply(text: string): void;
  onOpenThread(threadId: string): void;
  onShowSuggestions(): void;
  onOpenSettings(): void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const [searchMailbox, setSearchMailbox] = useState(false);
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

  const ask = (question: string) => {
    const text = question.trim();
    if (!text || pending) return;
    onAsk(text, searchMailbox);
    setDraft("");
    setSearchMailbox(false);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Escape") {
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
          {entry.searchMailbox ? <small>Searched all mail</small> : null}
        </p>
      ) : (
        <div key={entry.id} className="thread-chat-answer">
          <p>{entry.content}</p>
          {entry.replyDraft ? <div className="thread-chat-draft">
            <pre>{entry.replyDraft}</pre>
            <button type="button" onClick={() => onUseReply(entry.replyDraft!)}>Use as Reply</button>
          </div> : null}
          {entry.addedSuggestions > 0 ? <button type="button" className="context-link-button" onClick={onShowSuggestions}>
            Added {entry.addedSuggestions === 1 ? "1 suggestion" : `${entry.addedSuggestions} suggestions`} to review
          </button> : null}
          {entry.hiddenSuggestions > 0 ? <small>{entry.hiddenSuggestions === 1 ? "1 suggestion" : `${entry.hiddenSuggestions} suggestions`} couldn&rsquo;t be matched to the email, so {entry.hiddenSuggestions === 1 ? "it was" : "they were"} hidden.</small> : null}
          {entry.sources.length > 0 ? <nav className="thread-chat-sources" aria-label="Sources">
            {entry.sources.map((source) => <button type="button" key={source.threadId} onClick={() => onOpenThread(source.threadId)}>{source.subject || "(no subject)"}</button>)}
          </nav> : null}
          {entry.searched.length > 0 ? <details className="thread-chat-searched">
            <summary>Shared {entry.searched.length === 1 ? "1 other email" : `${entry.searched.length} other emails`} with AI</summary>
            {entry.searched.map((source) => <button type="button" key={source.threadId} onClick={() => onOpenThread(source.threadId)}>{source.subject || "(no subject)"}</button>)}
          </details> : null}
        </div>
      ))}
      {pending ? <p className="context-status" role="status">Thinking…</p> : null}
    </div> : pending ? <p className="context-status" role="status">Thinking…</p> : null}
    {error ? <div className="action-analysis-error" role="alert">
      <p>{error}</p>
      <div className="action-analysis-error-actions"><button type="button" onClick={onRetry}><RotateCcw size={13} /> Try Again</button></div>
    </div> : null}
    {!enabled || !available ? <>
      <p className="context-status">{!enabled ? "Turn on Thread Chat in AI settings to ask questions about this conversation." : "Set up an AI provider and API key in AI settings to ask questions."}</p>
      <button type="button" className="context-link-button" onClick={onOpenSettings}>AI Settings</button>
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
          rows={2}
          maxLength={2000}
          value={draft}
          placeholder="Ask about this conversation…"
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
        />
        <p id="thread-chat-hint" className="sr-only">Enter to ask, Shift+Enter for a new line, Escape to return to shortcuts.</p>
        {!draft.trim() && entries.length === 0 ? <div className="thread-chat-quick" aria-label="Suggested questions" role="group">
          {QUICK_QUESTIONS.map((question) => <button key={question} type="button" disabled={pending} onClick={() => ask(question)}>{question}</button>)}
        </div> : null}
        <div className="thread-chat-controls">
          <label title="Also search your other mail for this question only">
            <input type="checkbox" checked={searchMailbox} onChange={(event) => setSearchMailbox(event.target.checked)} />
            Search all mail
          </label>
          <button type="submit" disabled={pending || !draft.trim()}><Sparkles size={13} /> Ask</button>
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
