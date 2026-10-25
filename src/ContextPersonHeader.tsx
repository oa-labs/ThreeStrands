import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { ChevronDown, ChevronUp } from "lucide-react";
import { ICON_SIZE } from "./iconSizes";

/** Someone the person sections can be about. */
export type ContextPersonOption = {
  /** The address the panel selects when this person is picked. */
  email: string;
  /** Every address that counts as this person, such as a saved contact's. */
  emails?: string[];
  name: string | null;
};

/** Names the scope of contact files and recent emails, with optional participant switching. */
export function ContextPersonHeader({ people, selectedEmail, displayName, onSelect }: {
  people: ContextPersonOption[];
  selectedEmail: string;
  /** The selected person's name when better known than `people` says, such as a saved contact's. */
  displayName?: string | null;
  onSelect(email: string): void;
}) {
  const [open, setOpen] = useState(false);
  const listId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const matches = (person: ContextPersonOption) => (person.emails ?? [person.email]).includes(selectedEmail);
  const selected = people.find(matches);
  const others = people.filter((person) => !matches(person));
  // Someone new to choose from, or nobody left, closes the list.
  const othersKey = others.map((person) => person.email).join("\n");
  useEffect(() => { setOpen(false); }, [othersKey]);
  useEffect(() => {
    if (open) listRef.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [open]);
  if (!selectedEmail) return null;
  const name = displayName || selected?.name || selectedEmail;
  const close = () => { setOpen(false); toggleRef.current?.focus(); };
  const onListKeyDown = (event: KeyboardEvent<HTMLUListElement>) => {
    if (event.key === "Escape") { event.stopPropagation(); close(); return; }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button")];
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
  };
  return (
    <div className="context-person" role="group" aria-label="Showing history with">
      <div className="context-person-main">
        <span className="context-person-label">History with</span>
        {others.length > 0 ? (
          <button
            ref={toggleRef}
            type="button"
            className="context-person-switch"
            aria-expanded={open}
            aria-controls={listId}
            aria-label={`Show history with ${name}`}
            title={selectedEmail}
            onClick={() => setOpen(!open)}
          >
            <span className="context-person-name">{name}</span>
            {open ? <ChevronUp size={ICON_SIZE.xs} /> : <ChevronDown size={ICON_SIZE.xs} />}
          </button>
        ) : <span className="context-person-name" title={selectedEmail}>{name}</span>}
      </div>
      {open ? (
        <ul id={listId} ref={listRef} className="context-person-list" aria-label="Other people" onKeyDown={onListKeyDown}>
          {others.map((person) => (
            <li key={person.email}>
              <button type="button" className="context-person-option" title={person.email} onClick={() => { onSelect(person.email); close(); }}>
                <span className="context-person-name">{person.name || person.email}</span>
                {person.name && person.name !== person.email ? <small className="context-person-email">{person.email}</small> : null}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
