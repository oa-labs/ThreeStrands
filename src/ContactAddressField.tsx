import { useEffect, useId, useRef, useState, type ClipboardEvent, type KeyboardEvent } from "react";
import { flushSync } from "react-dom";
import { Check, Copy, Star, Users, X } from "lucide-react";
import { matchingGroups, memberCountLabel } from "./contactGroups";
import { mailClient } from "./data/client";
import type { ContactGroupRecipients, ContactProfile } from "./domain";
import { isEmailAddress, splitTypedAddresses } from "./emailAddress";
import { logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

const EMAIL_SEPARATOR_KEYS = new Set(["Enter", ",", ";", " "]);
const LINK_SEPARATOR_KEYS = new Set(["Enter", " "]);
const EMAIL_SEPARATORS = /[\s,;]+/;
const LINK_SEPARATORS = /\s+/;
const MAX_ADDRESS_SUGGESTIONS = 8;

type AddressSuggestion = { email: string; name: string | null };

/**
 * One option per address of each matching address-book contact, skipping
 * addresses already in the field.
 */
export function addressSuggestions(profiles: readonly ContactProfile[], values: readonly string[]): AddressSuggestion[] {
  const taken = new Set(values.map((value) => value.toLocaleLowerCase()));
  const options: AddressSuggestion[] = [];
  for (const profile of profiles) {
    for (const email of profile.addresses) {
      if (taken.has(email.toLocaleLowerCase())) continue;
      taken.add(email.toLocaleLowerCase());
      options.push({ email, name: profile.displayName?.trim() || null });
      if (options.length === MAX_ADDRESS_SUGGESTIONS) return options;
    }
  }
  return options;
}

function addAll(values: string[], text: string, kind: "email" | "link"): string[] {
  const next = [...values];
  for (const token of text.split(kind === "email" ? EMAIL_SEPARATORS : LINK_SEPARATORS)) {
    const value = token.trim();
    if (value && !next.some((existing) => kind === "email" ? existing.toLocaleLowerCase() === value.toLocaleLowerCase() : existing === value)) next.push(value);
  }
  return next;
}

/**
 * A contact's email addresses as removable badges. Typing an address and
 * pressing Enter, comma, semicolon, or space (or leaving the field) adds it;
 * pasting a list adds each address. Each badge can copy its address. The
 * first address is the primary one, used when the contact is emailed as part
 * of a group; any other can be made primary. The saved contact validates
 * addresses.
 */
export function ContactAddressField({ addresses, disabled, onChange }: {
  addresses: string[];
  disabled?: boolean;
  onChange(addresses: string[]): void;
}) {
  return <ContactMultiValueField kind="email" label="Emails" inputLabel="Email Addresses" primary values={addresses} disabled={disabled} onChange={onChange} />;
}

/**
 * A list of email addresses, such as meeting invitees, as removable badges
 * that commit the same way as a contact's addresses. Typing a name or address
 * suggests contact groups, then address-book contacts; choosing a contact
 * adds a badge with their name, and choosing a group adds one per member.
 * Spaces separate typed addresses but not names. Only email addresses become
 * badges: anything else stays in the field with a hint, and is reported
 * through `onPendingChange` so the form can refuse to submit with it.
 */
export function EmailBadgeField({ label, values, disabled, onChange, onPendingChange }: {
  label: string;
  values: string[];
  disabled?: boolean;
  onChange(values: string[]): void;
  /** The text typed but not yet a badge. */
  onPendingChange?(text: string): void;
}) {
  return <ContactMultiValueField kind="email" label={label} inputLabel={label} suggestContacts requireAddresses values={values} disabled={disabled} onChange={onChange} onPendingChange={onPendingChange} />;
}

export function ContactLinksField({ links, disabled, onChange }: {
  links: string[];
  disabled?: boolean;
  onChange(links: string[]): void;
}) {
  return <ContactMultiValueField kind="link" label="Links" inputLabel="Links" values={links} disabled={disabled} onChange={onChange} />;
}

function ContactMultiValueField({ kind, label, inputLabel, primary = false, suggestContacts = false, requireAddresses = false, values, disabled, onChange, onPendingChange }: {
  kind: "email" | "link";
  label: string;
  inputLabel: string;
  /** The first address is the contact's primary one, and any other can be promoted. */
  primary?: boolean;
  /** Suggest contact groups and address-book contacts as the user types. */
  suggestContacts?: boolean;
  /** Only email addresses become badges; other text stays in the field. */
  requireAddresses?: boolean;
  values: string[];
  disabled?: boolean;
  onChange(values: string[]): void;
  onPendingChange?(text: string): void;
}) {
  const isEmail = kind === "email";
  const itemName = isEmail ? "address" : "link";
  const labelId = useId();
  const listId = useId();
  const hintId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState("");
  // Committing the draft left text that is not an address; cleared by the next edit.
  const [rejected, setRejected] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Names of contacts chosen from suggestions, by lowercase address, for their badges.
  const [names, setNames] = useState<ReadonlyMap<string, string>>(new Map());
  const [profiles, setProfiles] = useState<ContactProfile[]>([]);
  const [groups, setGroups] = useState<ContactGroupRecipients[]>([]);
  // What choosing a group just added, until the next edit.
  const [groupNote, setGroupNote] = useState<string | null>(null);
  const groupsRequested = useRef(false);
  const [dismissed, setDismissed] = useState(true);
  const [activeIndex, setActiveIndex] = useState(0);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const requestId = useRef(0);
  useEffect(() => () => {
    if (copiedTimer.current) clearTimeout(copiedTimer.current);
    if (debounce.current) clearTimeout(debounce.current);
  }, []);

  // Groups come first: typing a group's name is a deliberate choice.
  const groupOptions = suggestContacts ? matchingGroups(groups, draft) : [];
  const contactOptions = suggestContacts && draft.trim() ? addressSuggestions(profiles, values) : [];
  const optionCount = groupOptions.length + contactOptions.length;
  const open = !dismissed && optionCount > 0;
  const active = Math.min(activeIndex, optionCount - 1);
  const badgeName = (value: string) => names.get(value.toLocaleLowerCase()) ?? value;
  // Badges that arrived from outside, such as a meeting suggestion's guests.
  const invalidValues = requireAddresses ? values.filter((value) => !isEmailAddress(value)) : [];
  const hint = rejected && draft.trim()
    ? `“${draft.trim()}” isn't an email address. Choose a contact or type a full address.`
    : invalidValues.length
      ? `${invalidValues.map((value) => `“${value}”`).join(", ")} ${invalidValues.length === 1 ? "isn't an email address" : "aren't email addresses"}. Remove ${invalidValues.length === 1 ? "it" : "them"} or type a full address.`
      : null;

  const lookUp = (text: string) => {
    if (!suggestContacts) return;
    if (debounce.current) clearTimeout(debounce.current);
    // Invalidate any request in flight, including on clear.
    const thisRequest = ++requestId.current;
    const token = text.trim();
    if (!token) {
      setProfiles([]);
      return;
    }
    debounce.current = setTimeout(() => {
      void mailClient.listContactProfiles(token, MAX_ADDRESS_SUGGESTIONS).then((result) => {
        if (thisRequest === requestId.current) setProfiles(result);
      }).catch(logBackgroundFailure("Contact suggestion lookup"));
    }, 150);
  };
  const loadGroups = () => {
    if (!suggestContacts || groupsRequested.current) return;
    groupsRequested.current = true;
    void mailClient.listContactGroupRecipients().then(setGroups).catch((reason) => {
      groupsRequested.current = false;
      logBackgroundFailure("Contact group lookup")(reason);
    });
  };
  const changeDraft = (text: string) => {
    setDraft(text);
    setRejected(false);
    onPendingChange?.(text);
    setGroupNote(null);
    setDismissed(false);
    setActiveIndex(0);
    lookUp(text);
  };

  const copy = async (value: string) => {
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(value);
      setCopied(value);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(null), 1500);
    } catch {
      setCopied(null);
      setCopyFailed(true);
    }
  };

  const commit = (text: string) => {
    if (!requireAddresses) {
      const next = addAll(values, text, kind);
      changeDraft("");
      if (next.length !== values.length) onChange(next);
      return;
    }
    const { addresses, rest } = splitTypedAddresses(text);
    const named = addresses.filter((address) => address.name);
    if (named.length) setNames((current) => {
      const next = new Map(current);
      for (const address of named) next.set(address.email.toLocaleLowerCase(), address.name!);
      return next;
    });
    const next = addAll(values, addresses.map((address) => address.email).join(" "), kind);
    if (rest !== draft) changeDraft(rest);
    setRejected(Boolean(rest));
    if (next.length !== values.length) onChange(next);
  };
  const choose = (suggestion: AddressSuggestion) => {
    if (suggestion.name) setNames((current) => new Map(current).set(suggestion.email.toLocaleLowerCase(), suggestion.name!));
    commit(suggestion.email);
    setDismissed(true);
    inputRef.current?.focus();
  };
  const chooseGroup = (group: ContactGroupRecipients) => {
    const taken = new Set(values.map((value) => value.toLocaleLowerCase()));
    const added = group.members.filter((member) => !taken.has(member.email.toLocaleLowerCase()));
    setNames((current) => {
      const next = new Map(current);
      for (const member of added) if (member.displayName?.trim()) next.set(member.email.toLocaleLowerCase(), member.displayName.trim());
      return next;
    });
    changeDraft("");
    if (added.length) onChange([...values, ...added.map((member) => member.email)]);
    const already = group.members.length - added.length;
    setGroupNote(`Added ${added.length} ${added.length === 1 ? "person" : "people"} from ${group.name}${already ? ` · ${already} already in ${label}` : ""}`);
    setDismissed(true);
    inputRef.current?.focus();
  };
  const chooseOption = (index: number) => {
    if (index < groupOptions.length) chooseGroup(groupOptions[index]);
    else choose(contactOptions[index - groupOptions.length]);
  };
  const makePrimary = (target: string) => onChange([target, ...values.filter((value) => value !== target)]);
  const remove = (target: string) => {
    onChange(values.filter((value) => value !== target));
    inputRef.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      // Commit before the form's submit shortcut reads the list, and let it bubble.
      flushSync(() => commit(draft));
      return;
    }
    if (open) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setActiveIndex((active + (event.key === "ArrowDown" ? 1 : -1) + optionCount) % optionCount);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        chooseOption(active);
        return;
      }
      if (event.key === "Escape") {
        // Close the suggestions, not the dialog.
        event.preventDefault();
        event.stopPropagation();
        setDismissed(true);
        return;
      }
    }
    // A space may be part of a name being looked up, but never of an address.
    if (event.key === " " && suggestContacts && draft.trim() && !draft.includes("@")) return;
    if ((isEmail ? EMAIL_SEPARATOR_KEYS : LINK_SEPARATOR_KEYS).has(event.key)) {
      event.preventDefault();
      commit(draft);
    } else if (event.key === "Backspace" && !draft && values.length) {
      event.preventDefault();
      onChange(values.slice(0, -1));
    }
  };
  const onPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const text = event.clipboardData.getData("text");
    if (!(isEmail ? EMAIL_SEPARATORS : LINK_SEPARATORS).test(text.trim())) return;
    event.preventDefault();
    commit(`${draft} ${text}`);
  };

  return (
    <div className={`contact-multi-value-field${suggestContacts ? " has-suggestions" : ""}`}>
      <span id={labelId}>{label}</span>
      <div className="contact-multi-value-box" onClick={(event) => { if (event.target === event.currentTarget) inputRef.current?.focus(); }}>
        {values.map((value, index) => (
          <span key={value} className={`recipient-chip contact-multi-value-chip${invalidValues.includes(value) ? " invalid" : ""}`}>
            <span className="recipient-chip-label" title={value}>{badgeName(value)}</span>
            {primary && values.length > 1 ? index === 0
              ? <span className="contact-primary-tag">Primary</span>
              : <button type="button" className="recipient-chip-remove" aria-label={`Make ${value} primary`} title="Make primary address" disabled={disabled} onClick={() => makePrimary(value)}>
                <Star size={ICON_SIZE.xs} aria-hidden="true" />
              </button> : null}
            <button type="button" className="recipient-chip-remove contact-multi-value-copy" aria-label={copied === value ? `Copied ${value}` : `Copy ${value}`} title={`Copy ${itemName}`} onClick={() => void copy(value)}>
              {copied === value ? <Check size={ICON_SIZE.xs} aria-hidden="true" /> : <Copy size={ICON_SIZE.xs} aria-hidden="true" />}
            </button>
            <button type="button" className="recipient-chip-remove" aria-label={`Remove ${badgeName(value)}`} disabled={disabled} onClick={() => remove(value)}>
              <X size={ICON_SIZE.xs} aria-hidden="true" />
            </button>
          </span>
        ))}
        <input
          ref={inputRef}
          className="contact-multi-value-input"
          aria-label={inputLabel}
          aria-describedby={hint ? `${labelId} ${hintId}` : labelId}
          aria-invalid={hint ? true : undefined}
          {...suggestContacts ? {
            "aria-autocomplete": "list" as const,
            "aria-expanded": open,
            "aria-controls": listId,
            "aria-activedescendant": open ? `${listId}-${active}` : undefined,
          } : {}}
          type="text"
          inputMode={isEmail ? "email" : "url"}
          {...SEARCH_INPUT_ATTRIBUTES}
          placeholder={values.length ? `Add another ${itemName}` : isEmail ? "name@example.com" : "https://example.com"}
          disabled={disabled}
          value={draft}
          onChange={(event) => changeDraft(event.target.value)}
          onFocus={loadGroups}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => { setDismissed(true); commit(draft); }}
        />
      </div>
      {open ? (
        <ul className="recipient-suggestions" role="listbox" id={listId} aria-label={`${label} suggestions`}>
          {groupOptions.map((group, index) => (
            <li
              key={`group:${group.id}`}
              id={`${listId}-${index}`}
              role="option"
              aria-label={`${group.name}, group of ${memberCountLabel(group.members.length)}`}
              aria-selected={index === active}
              className={`recipient-suggestion-group${index === active ? " active" : ""}`}
              onMouseEnter={() => setActiveIndex(index)}
              onMouseDown={(event) => { event.preventDefault(); chooseGroup(group); }}
            >
              <Users size={ICON_SIZE.xs} aria-hidden="true" />
              <span className="recipient-suggestion-name">
                {group.name}
                <small>{memberCountLabel(group.members.length)}</small>
              </span>
            </li>
          ))}
          {contactOptions.map((suggestion, contactIndex) => {
            const index = groupOptions.length + contactIndex;
            return (
            <li
              key={suggestion.email}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={index === active}
              className={index === active ? "active" : undefined}
              onMouseEnter={() => setActiveIndex(index)}
              // Before the input's blur commits the typed text.
              onMouseDown={(event) => { event.preventDefault(); choose(suggestion); }}
            >
              <span className="recipient-suggestion-name">
                {suggestion.name ?? suggestion.email}
                {suggestion.name ? <small>{suggestion.email}</small> : null}
              </span>
            </li>
            );
          })}
        </ul>
      ) : null}
      {hint ? <p id={hintId} className="recipient-field-hint" role="alert">{hint}</p> : null}
      {groupNote ? <p className="recipient-group-note" role="status">{groupNote}</p> : null}
      {copyFailed ? <span className="contact-sidebar-copy-status" role="status">Could not copy {isEmail ? "email address" : "link"}</span> : null}
    </div>
  );
}
