import { useEffect, useRef, useState } from "react";
import { Pin, PinOff } from "lucide-react";
import { mailClient } from "./data/client";
import type { ContactSuggestion } from "./domain";

type Props = {
  id: "to" | "cc" | "bcc";
  label: string;
  value: string;
  account: string;
  disabled: boolean;
  onChange(value: string): void;
};

// A single `<input>` holds the whole comma-joined address list; only the
// segment after the last comma is ever a live suggestion query or swapped
// out on selection.
function lastToken(value: string): string {
  return value.slice(value.lastIndexOf(",") + 1).trimStart();
}

function replaceLastToken(value: string, replacement: string): string {
  const prefix = value.slice(0, value.lastIndexOf(",") + 1);
  return `${prefix}${prefix ? " " : ""}${replacement}, `;
}

export function RecipientField({ id, label, value, account, disabled, onChange }: Props) {
  const [suggestions, setSuggestions] = useState<ContactSuggestion[]>([]);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(-1);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const requestId = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => () => { if (debounce.current) clearTimeout(debounce.current); }, []);

  function fetchSuggestions(token: string) {
    const thisRequest = ++requestId.current;
    void mailClient
      .listContactSuggestions(account, token, 8)
      .then((results) => {
        if (thisRequest !== requestId.current) return;
        setSuggestions(results);
        setOpen(results.length > 0);
        setActiveIndex(results.length > 0 ? 0 : -1);
      })
      .catch(() => {});
  }

  function query(nextValue: string) {
    if (debounce.current) clearTimeout(debounce.current);
    const token = lastToken(nextValue).trim();
    if (!token || !account) {
      setSuggestions([]);
      setOpen(false);
      return;
    }
    debounce.current = setTimeout(() => fetchSuggestions(token), 150);
  }

  function select(contact: ContactSuggestion) {
    const formatted = contact.displayName ? `${contact.displayName} <${contact.email}>` : contact.email;
    onChange(replaceLastToken(value, formatted));
    setOpen(false);
    setSuggestions([]);
    setActiveIndex(-1);
    inputRef.current?.focus();
  }

  function togglePin(contact: ContactSuggestion) {
    const action = contact.pinned
      ? mailClient.unpinContact(account, contact.email)
      : mailClient.pinContact(account, contact.email, contact.displayName);
    void action.then(() => fetchSuggestions(lastToken(value).trim()));
  }

  return (
    <label className="compose-field recipient-field">
      <span>{label}</span>
      <input
        ref={inputRef}
        name={id}
        aria-label={label}
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={`${id}-suggestions`}
        aria-activedescendant={open && activeIndex >= 0 ? `${id}-suggestion-${activeIndex}` : undefined}
        value={value}
        disabled={disabled}
        placeholder={id === "to" ? "Name <email@example.com>" : undefined}
        onChange={(event) => {
          onChange(event.target.value);
          query(event.target.value);
        }}
        onFocus={() => query(value)}
        onBlur={() => setOpen(false)}
        onKeyDown={(event) => {
          if (!open || suggestions.length === 0) return;
          if (event.key === "ArrowDown") {
            event.preventDefault();
            setActiveIndex((index) => (index + 1) % suggestions.length);
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            setActiveIndex((index) => (index - 1 + suggestions.length) % suggestions.length);
          } else if (event.key === "Enter" && activeIndex >= 0) {
            event.preventDefault();
            select(suggestions[activeIndex]);
          } else if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            setOpen(false);
          }
        }}
      />
      {open && (
        <ul className="recipient-suggestions" role="listbox" id={`${id}-suggestions`} aria-label={`${label} suggestions`}>
          {suggestions.map((contact, index) => (
            <li
              key={contact.email}
              id={`${id}-suggestion-${index}`}
              role="option"
              aria-selected={index === activeIndex}
              className={index === activeIndex ? "active" : undefined}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => {
                event.preventDefault();
                select(contact);
              }}
            >
              <span className="recipient-suggestion-name">
                {contact.displayName ?? contact.email}
                {contact.displayName && <small>{contact.email}</small>}
              </span>
              <button
                type="button"
                className="recipient-pin-toggle"
                aria-label={contact.pinned ? `Unpin ${contact.email}` : `Pin ${contact.email}`}
                aria-pressed={contact.pinned}
                onMouseDown={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                }}
                onClick={(event) => {
                  event.stopPropagation();
                  togglePin(contact);
                }}
              >
                {contact.pinned ? <Pin size={13} /> : <PinOff size={13} />}
              </button>
            </li>
          ))}
        </ul>
      )}
    </label>
  );
}
