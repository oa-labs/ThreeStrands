import { useId, useState, type KeyboardEvent } from "react";
import { MapPin } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";
import { useLocationSuggestions } from "./useLocationSuggestions";

/**
 * The Location field of the event dialogs. As the user types, it offers places
 * written on their other events; whatever they type is still saved as-is.
 * Suggestions appear only after the user edits the field, never for a
 * location the dialog opened with.
 */
export function EventLocationField({ value, disabled, onChange }: {
  value: string;
  disabled?: boolean;
  onChange(value: string): void;
}) {
  const listId = useId();
  const [edited, setEdited] = useState(false);
  const [dismissed, setDismissed] = useState(true);
  const [activeIndex, setActiveIndex] = useState(0);
  const suggestions = useLocationSuggestions(value, edited && !disabled);
  // A suggestion identical to the text adds nothing.
  const options = suggestions.filter((suggestion) => suggestion.toLocaleLowerCase() !== value.trim().toLocaleLowerCase());
  const open = !dismissed && options.length > 0;
  const active = Math.min(activeIndex, options.length - 1);

  const change = (text: string) => {
    setEdited(true);
    setDismissed(false);
    setActiveIndex(0);
    onChange(text);
  };
  const choose = (suggestion: string) => {
    onChange(suggestion);
    setDismissed(true);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (!open) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((active + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length);
    } else if (event.key === "Enter" && !event.metaKey && !event.ctrlKey) {
      // Choose rather than submit the form; Command-Return still saves.
      event.preventDefault();
      choose(options[active]);
    } else if (event.key === "Escape") {
      // Close the suggestions, not the dialog.
      event.preventDefault();
      event.stopPropagation();
      setDismissed(true);
    }
  };

  return (
    <div className="event-location-field">
      <label>Location<input
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open ? `${listId}-${active}` : undefined}
        {...SEARCH_INPUT_ATTRIBUTES}
        value={value}
        disabled={disabled}
        maxLength={4096}
        placeholder="Add location"
        onChange={(event) => change(event.target.value)}
        onKeyDown={onKeyDown}
        onBlur={() => setDismissed(true)}
      /></label>
      {open ? (
        <ul className="recipient-suggestions" role="listbox" id={listId} aria-label="Places from your calendar">
          {options.map((suggestion, index) => (
            <li
              key={suggestion}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={index === active}
              className={`event-location-suggestion${index === active ? " active" : ""}`}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => { event.preventDefault(); choose(suggestion); }}
            >
              <MapPin size={ICON_SIZE.xs} aria-hidden="true" />
              <span>{suggestion}</span>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
