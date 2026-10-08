import { useEffect, useId, useRef, useState } from "react";
import { Pin, PinOff, UserPlus, Users, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { ContactGroupRecipients, ContactSuggestion } from "./domain";
import { matchingGroups, memberCountLabel } from "./contactGroups";
import { formatAddress, looksLikeCompleteAddress, parseAddress, splitAddressList } from "./emailAddress";
import { logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";

type Props = {
  id: "to" | "cc" | "bcc";
  label: string;
  value: string;
  account: string;
  disabled: boolean;
  labelExpanded?: boolean;
  onLabelClick?(): void;
  onChange(value: string): void;
};

type Chip = { email: string; displayName: string | null };

// The one drag in flight in this window, if any. A module-level singleton
// (not React state) because it coordinates between sibling field instances
// that otherwise share no state — HTML5 drag events can't read `dataTransfer`
// during `dragover`, only at `drop`, so the source can't be told "did this
// land somewhere real?" any other way than the drop target calling back here.
let dragOrigin: { field: Props["id"]; remove(): void } | null = null;

function toChip(segment: string): Chip {
  const parsed = parseAddress(segment);
  return { email: parsed.email, displayName: parsed.name !== parsed.email ? parsed.name : null };
}

function formatChip(chip: Chip): string {
  return formatAddress(chip.displayName, chip.email);
}

function mergeChip(chips: Chip[], candidate: Chip): Chip[] {
  if (chips.some((chip) => chip.email.toLowerCase() === candidate.email.toLowerCase())) return chips;
  return [...chips, candidate];
}

// The field's whole value is still one comma-joined string (unchanged wire
// format — the backend and MIME builder never see chips, only this string).
// A value arriving from outside this component (reply/forward prefill, an
// account switch, ...) is fully-formed, so even a lone trailing segment with
// no comma after it should render as a chip if it looks like a complete
// address; a value this component is itself producing keystroke-by-keystroke
// never goes through this path (see the `lastEmitted` guard below), so a
// chip never collapses out from under someone mid-type.
// Splitting is quote-aware ("Doe, Jane" <jane@example.com> is one recipient)
// and rejoins an unquoted "Doe, Jane <jane@example.com>" from older drafts.
function parseExternalValue(value: string): { chips: Chip[]; draftText: string } {
  const segments = splitAddressList(value);
  const endsWithSeparator = /,\s*$/.test(value);
  const last = segments[segments.length - 1] ?? "";
  const lastIsComplete = endsWithSeparator || (last.length > 0 && looksLikeCompleteAddress(parseAddress(last).email));
  const committed = lastIsComplete ? segments : segments.slice(0, -1);
  const chips = committed.filter((segment) => segment.length > 0).map(toChip);
  return { chips, draftText: lastIsComplete ? "" : last };
}

function serialize(chips: Chip[], draftText: string): string {
  const formatted = chips.map(formatChip);
  if (!formatted.length) return draftText;
  return draftText ? `${formatted.join(", ")}, ${draftText}` : `${formatted.join(", ")}, `;
}

export function RecipientField({ id, label, value, account, disabled, labelExpanded, onLabelClick, onChange }: Props) {
  const inputId = useId();
  const lastEmitted = useRef(value);
  const [chips, setChips] = useState<Chip[]>(() => parseExternalValue(value).chips);
  const [draftText, setDraftText] = useState(() => parseExternalValue(value).draftText);
  const [suggestions, setSuggestions] = useState<ContactSuggestion[]>([]);
  const [dismissed, setDismissed] = useState(true);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [dragOver, setDragOver] = useState(false);
  const [groups, setGroups] = useState<ContactGroupRecipients[]>([]);
  // What choosing a group just added, until the next edit.
  const [groupNote, setGroupNote] = useState<string | null>(null);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const requestId = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => () => { if (debounce.current) clearTimeout(debounce.current); }, []);

  useEffect(() => {
    if (value === lastEmitted.current) return;
    const parsed = parseExternalValue(value);
    lastEmitted.current = value;
    setChips(parsed.chips);
    setDraftText(parsed.draftText);
  }, [value]);

  function emit(nextChips: Chip[], nextDraftText: string) {
    const serialized = serialize(nextChips, nextDraftText);
    lastEmitted.current = serialized;
    setChips(nextChips);
    setDraftText(nextDraftText);
    onChange(serialized);
  }

  function removeChip(target: Chip) {
    emit(chips.filter((chip) => chip.email.toLowerCase() !== target.email.toLowerCase()), draftText);
  }

  function commitDraftText() {
    const trimmed = draftText.trim();
    if (!trimmed) return;
    emit(mergeChip(chips, toChip(trimmed)), "");
  }

  const visibleSuggestions = suggestions.filter(
    (contact) => !chips.some((chip) => chip.email.toLowerCase() === contact.email.toLowerCase()),
  );
  const draftEmail = draftText.trim() ? parseAddress(draftText.trim()).email : "";
  const candidateEmail = draftEmail && looksLikeCompleteAddress(draftEmail) ? draftEmail : null;
  const alreadyKnown = candidateEmail
    ? visibleSuggestions.some((contact) => contact.email.toLowerCase() === candidateEmail.toLowerCase())
    : true;
  // Someone with zero mail history (a brand-new contact) never turns up from
  // `listContactSuggestions` on its own — this is the only way to pin them.
  const addCandidate: Chip | null = candidateEmail && !alreadyKnown ? toChip(draftText.trim()) : null;
  // Groups come first: typing a group's name is a deliberate choice, and
  // choosing one adds every member as a chip, so nothing downstream changes.
  const groupOptions = matchingGroups(groups, draftText);
  const contactOffset = groupOptions.length;
  const addIndex = contactOffset + visibleSuggestions.length;
  const optionCount = addIndex + (addCandidate ? 1 : 0);
  const visible = !dismissed && optionCount > 0;
  const effectiveActiveIndex = optionCount > 0 ? Math.min(Math.max(activeIndex, 0), optionCount - 1) : -1;

  function fetchSuggestions(forToken: string) {
    const thisRequest = ++requestId.current;
    void mailClient
      .listContactSuggestions(account, forToken, 8)
      .then((results) => {
        if (thisRequest !== requestId.current) return;
        setSuggestions(results);
      })
      .catch(logBackgroundFailure("Contact suggestion lookup"));
  }

  function query(nextDraftText: string) {
    if (debounce.current) clearTimeout(debounce.current);
    const token = nextDraftText.trim();
    if (!token || !account) {
      setSuggestions([]);
      return;
    }
    debounce.current = setTimeout(() => fetchSuggestions(token), 150);
  }

  function loadGroups() {
    void mailClient.listContactGroupRecipients().then(setGroups).catch(logBackgroundFailure("Contact group lookup"));
  }

  function selectGroup(group: ContactGroupRecipients) {
    let next = chips;
    for (const member of group.members) next = mergeChip(next, { email: member.email, displayName: member.displayName });
    const added = next.length - chips.length;
    const already = group.members.length - added;
    emit(next, "");
    setGroupNote(`Added ${added} ${added === 1 ? "person" : "people"} from ${group.name}${already ? ` · ${already} already in ${label}` : ""}`);
    setDismissed(true);
    setActiveIndex(-1);
    inputRef.current?.focus();
  }

  function select(contact: ContactSuggestion) {
    emit(mergeChip(chips, { email: contact.email, displayName: contact.displayName }), "");
    setDismissed(true);
    setActiveIndex(-1);
    inputRef.current?.focus();
  }

  function addContact(candidate: Chip) {
    emit(mergeChip(chips, candidate), "");
    void mailClient.pinContact(account, candidate.email, candidate.displayName);
    setDismissed(true);
    setActiveIndex(-1);
    inputRef.current?.focus();
  }

  function togglePin(contact: ContactSuggestion) {
    const action = contact.pinned
      ? mailClient.unpinContact(account, contact.email)
      : mailClient.pinContact(account, contact.email, contact.displayName);
    void action.then(() => fetchSuggestions(draftText.trim()));
  }

  return (
    <>
    <div className="compose-field recipient-field">
      {onLabelClick ? (
        <button type="button" className="recipient-field-label" aria-expanded={labelExpanded} title={labelExpanded ? "Click to hide Cc/Bcc" : "Click to show Cc/Bcc"} onClick={onLabelClick} disabled={disabled}>{label}</button>
      ) : (
        <label htmlFor={inputId}>{label}</label>
      )}
      <div
        className={`recipient-chip-row${dragOver ? " drag-over" : ""}`}
        onDragEnter={(event) => {
          if (disabled) return;
          event.preventDefault();
          setDragOver(true);
        }}
        onDragOver={(event) => {
          if (disabled) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
        }}
        onDragLeave={() => setDragOver(false)}
        onDrop={(event) => {
          event.preventDefault();
          setDragOver(false);
          if (disabled) return;
          const raw = event.dataTransfer.getData("application/x-threestrands-recipient");
          const origin = dragOrigin;
          dragOrigin = null;
          if (!raw || origin?.field === id) return;
          let dropped: Chip;
          try {
            dropped = JSON.parse(raw);
          } catch {
            return;
          }
          if (!chips.some((chip) => chip.email.toLowerCase() === dropped.email.toLowerCase())) {
            emit([...chips, dropped], draftText);
          }
          origin?.remove();
        }}
      >
        {chips.map((chip) => (
          <span
            key={chip.email}
            className="recipient-chip"
            draggable={!disabled}
            onDragStart={(event) => {
              dragOrigin = { field: id, remove: () => removeChip(chip) };
              event.dataTransfer.setData("application/x-threestrands-recipient", JSON.stringify(chip));
              event.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => {
              dragOrigin = null;
            }}
          >
            <span className="recipient-chip-label">{chip.displayName ?? chip.email}</span>
            {!disabled && (
              <button
                type="button"
                className="recipient-chip-remove"
                aria-label={`Remove ${chip.displayName ?? chip.email}`}
                onClick={() => removeChip(chip)}
              >
                <X size={ICON_SIZE.xs} />
              </button>
            )}
          </span>
        ))}
        <input
          id={inputId}
          ref={inputRef}
          name={id}
          aria-label={label}
          aria-autocomplete="list"
          aria-expanded={visible}
          aria-controls={`${id}-suggestions`}
          aria-activedescendant={
            visible && effectiveActiveIndex >= 0
              ? effectiveActiveIndex < contactOffset
                ? `${id}-group-${effectiveActiveIndex}`
                : effectiveActiveIndex < addIndex
                  ? `${id}-suggestion-${effectiveActiveIndex - contactOffset}`
                  : `${id}-suggestion-add`
              : undefined
          }
          value={draftText}
          disabled={disabled}
          placeholder={id === "to" && chips.length === 0 ? "Name <email@example.com>" : undefined}
          onChange={(event) => {
            emit(chips, event.target.value);
            setDismissed(false);
            setGroupNote(null);
            query(event.target.value);
          }}
          onFocus={() => {
            setDismissed(false);
            loadGroups();
            query(draftText);
          }}
          onBlur={() => {
            setDismissed(true);
            commitDraftText();
          }}
          onPaste={(event) => {
            const pasted = event.clipboardData.getData("text");
            if (!pasted.includes(",")) return;
            event.preventDefault();
            const segments = splitAddressList(`${draftText}${pasted}`)
              .map((segment) => segment.trim())
              .filter(Boolean);
            emit(segments.map(toChip).reduce(mergeChip, chips), "");
          }}
          onKeyDown={(event) => {
            if (event.key === ",") {
              event.preventDefault();
              commitDraftText();
              return;
            }
            if (event.key === "Backspace" && draftText === "" && chips.length > 0) {
              event.preventDefault();
              removeChip(chips[chips.length - 1]);
              return;
            }
            if (!visible) {
              if (event.key === "Enter" && draftText.trim()) {
                event.preventDefault();
                commitDraftText();
              }
              return;
            }
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setActiveIndex((optionCount + effectiveActiveIndex + 1) % optionCount);
            } else if (event.key === "ArrowUp") {
              event.preventDefault();
              setActiveIndex((optionCount + effectiveActiveIndex - 1) % optionCount);
            } else if (event.key === "Enter") {
              event.preventDefault();
              if (effectiveActiveIndex < contactOffset) selectGroup(groupOptions[effectiveActiveIndex]);
              else if (effectiveActiveIndex < addIndex) select(visibleSuggestions[effectiveActiveIndex - contactOffset]);
              else if (addCandidate) addContact(addCandidate);
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              setDismissed(true);
            }
          }}
        />
      </div>
      {visible && (
        <ul className="recipient-suggestions" role="listbox" id={`${id}-suggestions`} aria-label={`${label} suggestions`}>
          {groupOptions.map((group, index) => (
            <li
              key={`group:${group.id}`}
              id={`${id}-group-${index}`}
              role="option"
              aria-label={`${group.name}, group of ${memberCountLabel(group.members.length)}`}
              aria-selected={index === effectiveActiveIndex}
              className={`recipient-suggestion-group${index === effectiveActiveIndex ? " active" : ""}`}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => {
                event.preventDefault();
                selectGroup(group);
              }}
            >
              <Users size={ICON_SIZE.xs} />
              <span className="recipient-suggestion-name">
                {group.name}
                <small>{memberCountLabel(group.members.length)}</small>
              </span>
            </li>
          ))}
          {visibleSuggestions.map((contact, contactIndex) => {
            const index = contactOffset + contactIndex;
            return (
            <li
              key={contact.email}
              id={`${id}-suggestion-${contactIndex}`}
              role="option"
              aria-selected={index === effectiveActiveIndex}
              className={index === effectiveActiveIndex ? "active" : undefined}
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
                {contact.pinned ? <Pin size={ICON_SIZE.xs} /> : <PinOff size={ICON_SIZE.xs} />}
              </button>
            </li>
            );
          })}
          {addCandidate && (
            <li
              id={`${id}-suggestion-add`}
              role="option"
              aria-selected={addIndex === effectiveActiveIndex}
              className={`recipient-suggestion-add${addIndex === effectiveActiveIndex ? " active" : ""}`}
              onMouseEnter={() => setActiveIndex(addIndex)}
              onMouseDown={(event) => {
                event.preventDefault();
                addContact(addCandidate);
              }}
            >
              <UserPlus size={ICON_SIZE.xs} />
              <span>Pin {addCandidate.email} as a contact</span>
            </li>
          )}
        </ul>
      )}
    </div>
    {/* Below the field's row, so the chips stay aligned with the label. */}
    {groupNote && <p className="recipient-group-note" role="status">{groupNote}</p>}
    </>
  );
}
