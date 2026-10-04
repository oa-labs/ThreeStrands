import { CheckSquare, Paperclip, Square, Star } from "lucide-react";
import { memo, type RefObject } from "react";
import type { Thread } from "./domain";
import { formatDisplayName, parseAddress } from "./emailAddress";
import { formatMailTimestamp } from "./threadPresentation";
import { decodeHtmlEntities } from "./SafeMessage";
import { selectionGestureFor, type SelectionGesture } from "./threadSelection";

const MATCH_START = "\u0001";
const MATCH_END = "\u0002";

export function HighlightedSnippet({ thread }: { thread: Thread }) {
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

export const ThreadRow = memo(function ThreadRow({
  thread,
  selected,
  checked,
  accountColor,
  showAccount,
  onSelect,
  onToggleCheck,
  onSelectionGesture,
  rowRef,
  hasTask = false,
}: {
  thread: Thread;
  selected: boolean;
  checked: boolean;
  accountColor?: string;
  showAccount: boolean;
  onSelect(id: string): void;
  onToggleCheck(id: string): void;
  onSelectionGesture?(id: string, gesture: SelectionGesture): void;
  rowRef?: RefObject<HTMLButtonElement | null>;
  hasTask?: boolean;
}) {
  return (
    <button
      ref={rowRef}
      role="option"
      aria-selected={selected}
      className={`thread-row ${selected ? "selected" : ""}`}
      onMouseDown={(event) => {
        // Keep Shift-click from extending a text selection across rows.
        if (event.shiftKey && onSelectionGesture) event.preventDefault();
      }}
      onClick={(event) => {
        const gesture = onSelectionGesture ? selectionGestureFor(event) : null;
        if (gesture) onSelectionGesture!(thread.id, gesture);
        else onSelect(thread.id);
      }}
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
        {thread.hasAttachments ? <Paperclip className="thread-attachment" size={13} aria-label="Has attachments" /> : null}
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
            {hasTask ? <CheckSquare className="thread-task-indicator" size={13} aria-label="Has open task" /> : null}
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
