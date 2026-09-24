import { useEffect, useState, type ReactNode } from "react";

type OptionProps = {
  id: string;
  active: boolean;
  onMouseEnter(): void;
  onClick(): void;
};

export function FindOrCreatePicker<T>({
  items,
  getSearchText,
  placeholder,
  ariaLabel,
  listId,
  listLabel,
  emptyMessage,
  createLabel,
  onSelect,
  onCreate,
  renderItem,
  renderCreateItem,
}: {
  items: T[];
  getSearchText(item: T): string;
  placeholder: string;
  ariaLabel: string;
  listId: string;
  listLabel: string;
  emptyMessage: string;
  createLabel(query: string): ReactNode;
  onSelect(item: T): void;
  onCreate(query: string): void;
  renderItem(item: T, props: OptionProps): ReactNode;
  renderCreateItem?: (query: string, props: OptionProps) => ReactNode;
}) {
  const [query, setQuery] = useState("");
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const normalizedQuery = query.trim().toLowerCase();
  const filteredItems = normalizedQuery
    ? items.filter((item) => getSearchText(item).toLowerCase().includes(normalizedQuery))
    : items;
  const exactMatchExists = items.some((item) => getSearchText(item).toLowerCase() === normalizedQuery);
  const showCreateRow = normalizedQuery.length > 0 && !exactMatchExists;
  const rowCount = filteredItems.length + (showCreateRow ? 1 : 0);
  const activeIndex = rowCount === 0 ? -1 : Math.min(Math.max(highlightedIndex, 0), rowCount - 1);

  useEffect(() => setHighlightedIndex(0), [query]);

  const optionProps = (index: number, click: () => void): OptionProps => ({
    id: `${listId}-option-${index}`,
    active: index === activeIndex,
    onMouseEnter: () => setHighlightedIndex(index),
    onClick: click,
  });
  const selectRow = (index: number) => {
    if (index < filteredItems.length) onSelect(filteredItems[index]);
    else if (showCreateRow) onCreate(query.trim());
  };

  return (
    <>
      <div className="label-search">
        <input
          autoFocus
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={placeholder}
          aria-label={ariaLabel}
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
            }
          }}
        />
      </div>
      <div className="label-list" id={listId} role="listbox" aria-label={listLabel}>
        {filteredItems.map((item, index) => renderItem(item, optionProps(index, () => selectRow(index))))}
        {showCreateRow ? renderCreateItem
          ? renderCreateItem(query.trim(), optionProps(filteredItems.length, () => onCreate(query.trim())))
          : <div
              id={`${listId}-option-${filteredItems.length}`}
              role="option"
              aria-selected={filteredItems.length === activeIndex}
              className={filteredItems.length === activeIndex ? "highlighted" : undefined}
              onMouseEnter={() => setHighlightedIndex(filteredItems.length)}
              onClick={() => onCreate(query.trim())}
            >
              {createLabel(query.trim())}
            </div>
          : null}
        {filteredItems.length === 0 && !showCreateRow ? <p className="empty">{emptyMessage}</p> : null}
      </div>
    </>
  );
}
