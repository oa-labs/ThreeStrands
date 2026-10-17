import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { AiModelInfo } from "./aiSettings";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

/**
 * A combobox for choosing an AI model: a text input that filters a fetched
 * list as the user types, with keyboard navigation, and that always accepts a
 * value the list does not contain. The free-text path is deliberate — a user
 * may name a model newer than the fetched catalog, or one the provider's list
 * endpoint omits — so this never locks the value to the list the way a plain
 * `<select>` would.
 *
 * It implements the ARIA combobox pattern (editable, list autocomplete):
 * `role="combobox"` on the input, `aria-expanded`, `aria-controls`, and
 * `aria-activedescendant` pointing at the highlighted option in the listbox.
 */
export function ModelCombobox({
  value,
  models,
  placeholder,
  disabled = false,
  ariaLabel,
  onCommit,
}: {
  /** The committed model id (what is saved and sent). */
  value: string;
  /** The provider's catalog; may be empty before a fetch or when it failed. */
  models: AiModelInfo[];
  placeholder?: string;
  disabled?: boolean;
  ariaLabel: string;
  /** Called with the chosen id on selection or when the field loses focus. */
  onCommit(model: string): void;
}) {
  const listboxId = useId();
  const optionIdPrefix = useId();
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // `draft` is the live input text; it is only committed on select or blur so
  // typing a longer id does not fire a save per keystroke.
  const [draft, setDraft] = useState(value);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);

  // Follow the committed value only when it changes (a commit, or a provider
  // switch). Closing the list must not reset the draft: an outside click closes
  // it before the input's blur commits, and that commit needs the typed text.
  const [syncedValue, setSyncedValue] = useState(value);
  if (value !== syncedValue) {
    setSyncedValue(value);
    setDraft(value);
  }

  const filtered = useMemo(() => {
    const query = draft.trim().toLowerCase();
    const match = (model: AiModelInfo) =>
      model.id.toLowerCase().includes(query) ||
      (model.label ?? "").toLowerCase().includes(query);
    return query ? models.filter(match) : models;
  }, [draft, models]);

  // Close on an outside click so the list does not linger over other controls.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  const commit = (next: string) => {
    const trimmed = next.trim();
    setDraft(trimmed);
    setOpen(false);
    setActiveIndex(-1);
    if (trimmed !== value) onCommit(trimmed);
  };

  const select = (model: AiModelInfo) => {
    commit(model.id);
    inputRef.current?.focus();
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (!open) {
          setOpen(true);
          setActiveIndex(filtered.length > 0 ? 0 : -1);
        } else if (filtered.length > 0) {
          setActiveIndex((index) => (index + 1) % filtered.length);
        }
        break;
      case "ArrowUp":
        event.preventDefault();
        if (open && filtered.length > 0) {
          setActiveIndex((index) => (index <= 0 ? filtered.length - 1 : index - 1));
        }
        break;
      case "Enter":
        if (open && activeIndex >= 0 && activeIndex < filtered.length) {
          event.preventDefault();
          select(filtered[activeIndex]);
        } else {
          commit(draft);
        }
        break;
      case "Escape":
        if (open) {
          event.preventDefault();
          setDraft(value);
          setOpen(false);
          setActiveIndex(-1);
        }
        break;
      default:
        break;
    }
  };

  const activeOptionId =
    open && activeIndex >= 0 && activeIndex < filtered.length
      ? `${optionIdPrefix}-${activeIndex}`
      : undefined;

  return (
    <div className="model-combobox" ref={containerRef}>
      <input
        ref={inputRef}
        type="text"
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={open}
        aria-controls={listboxId}
        aria-autocomplete="list"
        aria-activedescendant={activeOptionId}
        {...SEARCH_INPUT_ATTRIBUTES}
        disabled={disabled}
        placeholder={placeholder}
        value={draft}
        onChange={(event) => {
          setDraft(event.target.value);
          setOpen(true);
          setActiveIndex(-1);
        }}
        onFocus={() => setOpen(true)}
        onKeyDown={onKeyDown}
        onBlur={() => commit(draft)}
      />
      {open && filtered.length > 0 ? (
        <ul className="model-combobox-list" role="listbox" id={listboxId} aria-label={ariaLabel}>
          {filtered.map((model, index) => (
            <li
              key={model.id}
              id={`${optionIdPrefix}-${index}`}
              role="option"
              aria-selected={model.id === value}
              className={`model-combobox-option${index === activeIndex ? " active" : ""}${model.id === value ? " selected" : ""}`}
              // onMouseDown, not onClick: mousedown fires before the input's
              // blur, so the selection lands before blur can commit stale text.
              onMouseDown={(event) => {
                event.preventDefault();
                select(model);
              }}
              onMouseEnter={() => setActiveIndex(index)}
            >
              <span className="model-combobox-id">{model.id}</span>
              {model.label && model.label !== model.id ? (
                <span className="model-combobox-label">{model.label}</span>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
