import { AtSign, Calendar, Clock, Folder, Inbox, Search, SlidersHorizontal, Tag, User } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type RefObject } from "react";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import { readRecentSearches, recordRecentSearch } from "./recentSearches";
import { logBackgroundFailure } from "./errors";
import {
  applySuggestion,
  contactQueryFor,
  searchSuggestions,
  tokenAt,
  type SearchSuggestion,
  type SuggestionContact,
} from "./searchSuggestions";

/** How long typing must pause before a non-empty search is committed. */
export const SEARCH_DEBOUNCE_MS = 180;
/** How long a `from:`/`to:` value must hold still before contacts are looked up. */
export const CONTACT_SUGGESTION_DEBOUNCE_MS = 120;

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
  /** User label names offered after `label:` and `in:`. */
  labels?: readonly string[];
  /** Contacts matching a `from:`/`to:` value, best first. */
  loadContacts?: (query: string) => Promise<SuggestionContact[]>;
};

const NO_LABELS: readonly string[] = [];

const SUGGESTION_ICONS = {
  operator: SlidersHorizontal,
  contact: User,
  label: Tag,
  location: Folder,
  value: AtSign,
  date: Calendar,
  recent: Clock,
} as const;

/**
 * The mailbox search input. Keystrokes update only this component's draft;
 * the query is committed to the app after typing pauses, so the rest of the
 * window re-renders once per search instead of once per keystroke. Clearing
 * the field commits immediately.
 *
 * Below the input, a suggestion list completes search operators and their
 * values (contacts, labels, locations, dates) and offers recent searches when
 * the field is empty. Arrow keys move through it, Enter or a click chooses,
 * and Escape closes it before it clears the search.
 */
export function SearchField({
  inputRef,
  query,
  onCommit,
  onInput,
  onEscape,
  inboxOnly,
  onToggleInboxOnly,
  labels = NO_LABELS,
  loadContacts,
}: SearchFieldProps) {
  const listId = useId();
  const [draft, setDraft] = useState(query);
  const [caret, setCaret] = useState(query.length);
  const committedRef = useRef(query);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const [contacts, setContacts] = useState<SuggestionContact[]>([]);
  const [recentSearches, setRecentSearches] = useState(readRecentSearches);
  const contactRequest = useRef(0);
  const pendingCaret = useRef<number | null>(null);

  // Adopt a query changed elsewhere (a mailbox switch clearing search).
  useEffect(() => {
    if (query === committedRef.current) return;
    committedRef.current = query;
    setDraft(query);
    setCaret(query.length);
  }, [query]);

  useEffect(() => {
    if (draft === committedRef.current) return;
    const timeout = window.setTimeout(() => {
      committedRef.current = draft;
      onCommit(draft);
    }, draft.trim() ? SEARCH_DEBOUNCE_MS : 0);
    return () => window.clearTimeout(timeout);
  }, [draft, onCommit]);

  // Restore the caret after a suggestion rewrites the draft.
  useLayoutEffect(() => {
    if (pendingCaret.current === null) return;
    inputRef.current?.setSelectionRange(pendingCaret.current, pendingCaret.current);
    pendingCaret.current = null;
  }, [draft, inputRef]);

  const token = useMemo(() => tokenAt(draft, caret), [draft, caret]);
  const contactQuery = focused && loadContacts ? contactQueryFor(token) : null;

  useEffect(() => {
    const request = ++contactRequest.current;
    if (contactQuery === null || !loadContacts) {
      setContacts([]);
      return;
    }
    const timeout = window.setTimeout(() => {
      loadContacts(contactQuery)
        .then((found) => {
          if (request === contactRequest.current) setContacts(found);
        })
        .catch(logBackgroundFailure("Search contact suggestions"));
    }, CONTACT_SUGGESTION_DEBOUNCE_MS);
    return () => window.clearTimeout(timeout);
  }, [contactQuery, loadContacts]);

  const suggestions = useMemo(
    () => searchSuggestions(draft, token, { labels, contacts, recentSearches, now: new Date() }),
    [draft, token, labels, contacts, recentSearches],
  );
  const visible = focused && !dismissed && suggestions.length > 0;
  // An operator's values are what the user is reaching for, so the first is
  // ready for Enter; a bare word or a recent search waits for an arrow key.
  const defaultIndex = token.operator ? 0 : -1;
  const effectiveIndex = visible ? Math.min(activeIndex ?? defaultIndex, suggestions.length - 1) : -1;

  function updateDraft(next: string, nextCaret: number) {
    onInput();
    setDraft(next);
    setCaret(nextCaret);
    setDismissed(false);
    setActiveIndex(null);
  }

  function commitNow(next: string) {
    if (next === committedRef.current) return;
    committedRef.current = next;
    onCommit(next);
  }

  function rememberSearch() {
    if (committedRef.current.trim()) setRecentSearches(recordRecentSearch(committedRef.current));
  }

  function choose(suggestion: SearchSuggestion) {
    const next = applySuggestion(draft, token, suggestion);
    pendingCaret.current = next.caret;
    updateDraft(next.query, next.caret);
    if (suggestion.kind === "recent") commitNow(next.query);
  }

  return (
    <div className="search-field">
      <label className="search-box">
        <Search size={ICON_SIZE.md} />
        <input
          ref={inputRef}
          {...SEARCH_INPUT_ATTRIBUTES}
          value={draft}
          onChange={(event) => updateDraft(event.target.value, event.target.selectionStart ?? event.target.value.length)}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart ?? event.currentTarget.value.length)}
          onFocus={() => setFocused(true)}
          onBlur={() => {
            setFocused(false);
            setDismissed(false);
            setActiveIndex(null);
            rememberSearch();
          }}
          placeholder="Search mail"
          aria-label="Search Mail"
          aria-autocomplete="list"
          aria-controls={visible ? listId : undefined}
          aria-activedescendant={effectiveIndex >= 0 ? `${listId}-${effectiveIndex}` : undefined}
          data-mailbox-tab-shortcut
          data-shortcut-scope="search"
          onKeyDown={(event) => {
            if (visible && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
              event.preventDefault();
              const step = event.key === "ArrowDown" ? 1 : -1;
              const from = effectiveIndex < 0 && step < 0 ? suggestions.length : effectiveIndex;
              setActiveIndex((from + step + suggestions.length) % suggestions.length);
              return;
            }
            if (event.key === "Enter") {
              event.preventDefault();
              if (effectiveIndex >= 0) {
                choose(suggestions[effectiveIndex]);
                return;
              }
              commitNow(draft);
              rememberSearch();
              setDismissed(true);
              return;
            }
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            if (visible && draft.trim()) {
              setDismissed(true);
              return;
            }
            committedRef.current = "";
            setDraft("");
            setCaret(0);
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
      {visible ? (
        <ul className="search-suggestions" role="listbox" id={listId} aria-label={draft.trim() ? "Search suggestions" : "Recent searches"}>
          {suggestions.map((suggestion, index) => {
            const Icon = SUGGESTION_ICONS[suggestion.kind];
            return (
              <li
                key={suggestion.key}
                id={`${listId}-${index}`}
                role="option"
                aria-selected={index === effectiveIndex}
                className={index === effectiveIndex ? "active" : undefined}
                // Keep focus in the input so choosing doesn't blur the search.
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => choose(suggestion)}
              >
                <Icon size={ICON_SIZE.sm} aria-hidden="true" />
                <span className="search-suggestion-label">{suggestion.label}</span>
                {suggestion.detail ? <small>{suggestion.detail}</small> : null}
              </li>
            );
          })}
        </ul>
      ) : null}
    </div>
  );
}
