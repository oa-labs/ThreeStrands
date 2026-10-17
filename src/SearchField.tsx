import { Inbox, Search } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

/** How long typing must pause before a non-empty search is committed. */
export const SEARCH_DEBOUNCE_MS = 180;

type SearchFieldProps = {
  inputRef: RefObject<HTMLInputElement | null>;
  /** The committed query that drives the thread list. */
  query: string;
  onCommit: (query: string) => void;
  /** Runs on every keystroke, before the debounced commit. */
  onInput: () => void;
  onEscape: () => void;
  /** Narrows the search from all mail (the default) to the inbox. */
  inboxOnly: boolean;
  onToggleInboxOnly: () => void;
};

/**
 * The mailbox search input. Keystrokes update only this component's draft;
 * the query is committed to the app after typing pauses, so the rest of the
 * window re-renders once per search instead of once per keystroke. Clearing
 * the field commits immediately.
 */
export function SearchField({
  inputRef,
  query,
  onCommit,
  onInput,
  onEscape,
  inboxOnly,
  onToggleInboxOnly,
}: SearchFieldProps) {
  const [draft, setDraft] = useState(query);
  const committedRef = useRef(query);

  // Adopt a query changed elsewhere (a mailbox switch clearing search).
  useEffect(() => {
    if (query === committedRef.current) return;
    committedRef.current = query;
    setDraft(query);
  }, [query]);

  useEffect(() => {
    if (draft === committedRef.current) return;
    const timeout = window.setTimeout(() => {
      committedRef.current = draft;
      onCommit(draft);
    }, draft.trim() ? SEARCH_DEBOUNCE_MS : 0);
    return () => window.clearTimeout(timeout);
  }, [draft, onCommit]);

  return (
    <label className="search-box">
      <Search size={ICON_SIZE.md} />
      <input
        ref={inputRef}
        {...SEARCH_INPUT_ATTRIBUTES}
        value={draft}
        onChange={(event) => {
          onInput();
          setDraft(event.target.value);
        }}
        placeholder="Search mail"
        aria-label="Search Mail"
        data-mailbox-tab-shortcut
        data-shortcut-scope="search"
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          committedRef.current = "";
          setDraft("");
          onEscape();
        }}
      />
      {draft.trim() ? (
        <button
          type="button"
          className={`search-toggle ${inboxOnly ? "active" : ""}`}
          aria-pressed={inboxOnly}
          aria-label="Search only the Inbox"
          title={inboxOnly ? "Searching only the Inbox. Click to search all mail." : "Searching all mail. Click to search only the Inbox."}
          onClick={onToggleInboxOnly}
        >
          <Inbox size={ICON_SIZE.sm} />
          <span>Inbox Only</span>
        </button>
      ) : null}
      <kbd>/</kbd>
    </label>
  );
}
