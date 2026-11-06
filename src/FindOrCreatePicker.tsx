import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

type OptionProps = {
  id: string;
  active: boolean;
  onMouseEnter(): void;
  onClick(): void;
};

export function FindOrCreatePicker<T>({
  items,
  getSearchText,
  getItemKey,
  placeholder,
  ariaLabel,
  ariaDescribedBy,
  searchHint,
  multiple = false,
  renderActiveItemDetails,
  listId,
  listLabel,
  emptyMessage,
  createLabel,
  onSelect,
  onCreate,
  renderItem,
  renderCreateItem,
  isExactMatch,
  notice,
  tokens,
  onRemoveLastToken,
  clearOnPick = false,
  allowCreate = true,
}: {
  items: T[];
  getSearchText(item: T): string;
  getItemKey?(item: T): string;
  placeholder: string;
  ariaLabel: string;
  ariaDescribedBy?: string;
  searchHint?: ReactNode;
  multiple?: boolean;
  /** Visible details for the keyboard-highlighted or hovered item; also read by the combobox. */
  renderActiveItemDetails?(item: T): ReactNode;
  listId: string;
  listLabel: string;
  emptyMessage: string;
  createLabel(query: string): ReactNode;
  onSelect(item: T, query: string): void;
  /** Returning false keeps the query, such as when the typed text is rejected. */
  onCreate(query: string): boolean | void;
  renderItem(item: T, props: OptionProps): ReactNode;
  renderCreateItem?: (query: string, props: OptionProps) => ReactNode;
  /** Whether an item already is what the query names; such an item replaces the create row. */
  isExactMatch?: (item: T, query: string) => boolean;
  /** Explains why the query can't be created; shown instead of the create row. */
  notice?: (query: string) => ReactNode;
  /** Chosen items, shown inside the search field ahead of the input. */
  tokens?: ReactNode;
  /** Backspace in an empty field removes the last token. */
  onRemoveLastToken?: () => void;
  /** Clears the query after an item is chosen or created. */
  clearOnPick?: boolean;
  /** Hide creation for catalogs that only allow selecting existing items. */
  allowCreate?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const names = (item: T, text: string) => isExactMatch
    ? isExactMatch(item, text.trim())
    : getSearchText(item).toLowerCase() === text.trim().toLowerCase();
  const itemsFor = (text: string) => {
    const normalized = text.trim().toLowerCase();
    return normalized
      ? items.filter((item) => getSearchText(item).toLowerCase().includes(normalized) || names(item, text))
      : items;
  };
  const normalizedQuery = query.trim().toLowerCase();
  const matches = (item: T) => names(item, query);
  const filteredItems = itemsFor(query);
  const exactMatchExists = normalizedQuery.length > 0 && items.some(matches);
  const noticeContent = normalizedQuery ? notice?.(query.trim()) : null;
  const showCreateRow = allowCreate && normalizedQuery.length > 0 && !exactMatchExists && !noticeContent;
  const rowCount = filteredItems.length + (showCreateRow ? 1 : 0);
  const activeIndex = rowCount === 0 ? -1 : Math.min(Math.max(highlightedIndex, 0), rowCount - 1);

  const activeDetails = activeIndex >= 0 && activeIndex < filteredItems.length
    ? renderActiveItemDetails?.(filteredItems[activeIndex]) : null;
  const hasActiveDetails = Boolean(activeDetails);
  useLayoutEffect(() => {
    if (multiple && activeIndex >= 0) document.getElementById(`${listId}-option-${activeIndex}`)?.scrollIntoView?.({ block: "nearest" });
  }, [multiple, listId, activeIndex, hasActiveDetails]);

  const previous = useRef<{ query: string; items: T[] } | null>(null);
  useLayoutEffect(() => {
    const before = previous.current;
    previous.current = { query, items: filteredItems };
    if (!getItemKey || !before || before.query !== query) return;
    const oldItem = before.items[highlightedIndex];
    if (!oldItem) return;
    const index = filteredItems.findIndex((item) => getItemKey(item) === getItemKey(oldItem));
    if (index !== highlightedIndex) setHighlightedIndex(Math.max(index, 0));
  }, [query, filteredItems, getItemKey, highlightedIndex]);

  // A new query starts on the item it names exactly, so Enter picks it.
  const changeQuery = (next: string) => {
    setQuery(next);
    setHighlightedIndex(next.trim() ? Math.max(itemsFor(next).findIndex((item) => names(item, next)), 0) : 0);
  };

  const optionProps = (index: number, click: () => void): OptionProps => ({
    id: `${listId}-option-${index}`,
    active: index === activeIndex,
    onMouseEnter: () => setHighlightedIndex(index),
    onClick: () => { setHighlightedIndex(index); click(); },
  });
  const create = () => {
    if (onCreate(query.trim()) !== false && clearOnPick) changeQuery("");
  };
  const selectRow = (index: number) => {
    if (index < filteredItems.length) {
      onSelect(filteredItems[index], query.trim());
      if (clearOnPick && (!multiple || query)) changeQuery("");
    } else if (showCreateRow) create();
  };
  const input = <input
    {...SEARCH_INPUT_ATTRIBUTES}
    ref={inputRef}
    autoFocus
    value={query}
    onChange={(event) => changeQuery(event.target.value)}
    placeholder={tokens ? undefined : placeholder}
    aria-label={ariaLabel}
    aria-describedby={[ariaDescribedBy, activeDetails ? `${listId}-details` : null].filter(Boolean).join(" ") || undefined}
    role="combobox"
    aria-expanded="true"
    aria-controls={listId}
    aria-activedescendant={activeIndex >= 0 ? `${listId}-option-${activeIndex}` : undefined}
    onKeyDown={(event) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setHighlightedIndex((index) => Math.min(index + 1, Math.max(rowCount - 1, 0)));
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        setHighlightedIndex((index) => Math.max(index - 1, 0));
      } else if (event.key === "Enter") {
        event.preventDefault();
        if (activeIndex >= 0) selectRow(activeIndex);
      } else if (event.key === "Backspace" && !query && tokens && onRemoveLastToken) {
        event.preventDefault();
        onRemoveLastToken();
      }
    }}
  />;

  return (
    <>
      <div className="label-search">
        {tokens || multiple
          ? <div className="find-or-create-field" onClick={(event) => { if (event.target === event.currentTarget) inputRef.current?.focus(); }}>{tokens}{input}</div>
          : input}
        {searchHint}
      </div>
      <div className="label-list" id={listId} role="listbox" aria-label={listLabel} aria-multiselectable={multiple || undefined}>
        {filteredItems.map((item, index) => renderItem(item, optionProps(index, () => selectRow(index))))}
        {showCreateRow ? renderCreateItem
          ? renderCreateItem(query.trim(), optionProps(filteredItems.length, create))
          : <div
              id={`${listId}-option-${filteredItems.length}`}
              role="option"
              aria-selected={filteredItems.length === activeIndex}
              className={filteredItems.length === activeIndex ? "highlighted" : undefined}
              onMouseEnter={() => setHighlightedIndex(filteredItems.length)}
              onClick={create}
            >
              {createLabel(query.trim())}
            </div>
          : null}
        {noticeContent ? <p className="find-or-create-notice" role="status">{noticeContent}</p> : null}
        {filteredItems.length === 0 && !showCreateRow && !noticeContent ? <p className="empty">{emptyMessage}</p> : null}
      </div>
      {activeDetails ? <div className="find-or-create-details" id={`${listId}-details`}>{activeDetails}</div> : null}
    </>
  );
}
