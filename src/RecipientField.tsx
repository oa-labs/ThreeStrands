import { useEffect, useEffectEvent, useId, useRef, useState, type ClipboardEvent as ReactClipboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { Pin, PinOff, UserPlus, Users, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { ContactGroupRecipients, ContactSuggestion } from "./domain";
import { matchingGroups, memberCountLabel } from "./contactGroups";
import { formatAddress, isEmailAddress, isValidRecipient, looksLikeCompleteAddress, normalizePastedEmailAddresses, parseAddress, splitAddressList, splitTypedAddresses } from "./emailAddress";
import { logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

type Props = {
  id: "to" | "cc" | "bcc";
  label: string;
  value: string;
  account: string;
  disabled: boolean;
  context?: import("./domain").RecipientContext;
  labelExpanded?: boolean;
  onLabelClick?(): void;
  onChange(value: string): void;
  onCommittedChange?(field: Props["id"], emails: string[]): void;
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

function chipKey(chip: Chip): string {
  return chip.email.toLowerCase();
}

function isChip(value: unknown): value is Chip {
  if (typeof value !== "object" || value === null) return false;
  const { email, displayName } = value as Record<string, unknown>;
  return typeof email === "string" && (displayName === null || typeof displayName === "string");
}

// A drag carries one chip, or every selected chip as an array.
function parseDroppedChips(raw: string): Chip[] | null {
  try {
    const parsed: unknown = JSON.parse(raw);
    const list: unknown[] = Array.isArray(parsed) ? parsed : [parsed];
    return list.length > 0 && list.every(isChip) ? list : null;
  } catch {
    return null;
  }
}

function mergeTyped(chips: Chip[], addresses: { email: string; name: string | null }[]): Chip[] {
  return addresses.reduce((next, address) => mergeChip(next, { email: address.email, displayName: address.name }), chips);
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

export function completeRecipientEmails(value: string): string[] {
  return parseExternalValue(value).chips.map((chip) => chip.email.toLowerCase()).filter(isEmailAddress);
}

function serialize(chips: Chip[], draftText: string): string {
  const formatted = chips.map(formatChip);
  if (!formatted.length) return draftText;
  return draftText ? `${formatted.join(", ")}, ${draftText}` : `${formatted.join(", ")}, `;
}

export function RecipientField({ id, label, value, account, disabled, context, labelExpanded, onLabelClick, onChange, onCommittedChange }: Props) {
  const inputId = useId();
  const lastEmitted = useRef(value);
  const [chips, setChips] = useState<Chip[]>(() => parseExternalValue(value).chips);
  const [draftText, setDraftText] = useState(() => parseExternalValue(value).draftText);
  // Committing left text that is not an address; cleared by the next edit. A
  // reopened draft's leftover text was never committed, so it starts flagged.
  const [rejected, setRejected] = useState(() => Boolean(parseExternalValue(value).draftText.trim()));
  const hintId = useId();
  const [suggestions, setSuggestions] = useState<ContactSuggestion[]>([]);
  const [dismissed, setDismissed] = useState(true);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [dragOver, setDragOver] = useState(false);
  const [groups, setGroups] = useState<ContactGroupRecipients[]>([]);
  // Selected chips for cut/copy/delete/drag; emails lowercased. Cleared when
  // the draft text changes, the input loses focus, or the field's committed
  // value is replaced from outside.
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  // The fixed end of a Shift-extended selection, and the end arrow keys move.
  const selectionAnchor = useRef<string | null>(null);
  const selectionCursor = useRef<string | null>(null);
  // A press on a chip that will select it on click. The press blurs the input
  // and the click refocuses it; neither should clear the selection or open
  // suggestions.
  const chipPointer = useRef(false);
  // What choosing a group just added, until the next edit.
  const [groupNote, setGroupNote] = useState<string | null>(null);
  const debounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const requestId = useRef(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const seeds = context?.seeds ?? (id === "bcc" ? [] : chips.map((chip) => chip.email));
  const excluded = context?.excluded ?? chips.map((chip) => chip.email);
  const contextKey = JSON.stringify([seeds, excluded]);
  useEffect(() => {
    onCommittedChange?.(id, chips.map((chip) => chip.email.toLowerCase()).filter(isEmailAddress));
  }, [id, chips, onCommittedChange]);
  // Context changes invalidate before the next debounce, even in sibling fields.
  // query reads the current context and draft; text edits have their own immediate path.
  const requery = useEffectEvent(() => {
    if (document.activeElement === inputRef.current) query(draftText);
  });
  useEffect(() => {
    if (debounce.current) clearTimeout(debounce.current);
    ++requestId.current;
    setSuggestions([]);
    requery();
  }, [account, contextKey]);
  const groupsLoaded = useRef(false);
  const groupsLoading = useRef(false);

  useEffect(() => () => { ++requestId.current; if (debounce.current) clearTimeout(debounce.current); }, []);

  useEffect(() => {
    if (value === lastEmitted.current) return;
    ++requestId.current;
    if (debounce.current) clearTimeout(debounce.current);
    setSuggestions([]);
    const parsed = parseExternalValue(value);
    lastEmitted.current = value;
    setChips(parsed.chips);
    setDraftText(parsed.draftText);
    setRejected(Boolean(parsed.draftText.trim()));
    setSelected(new Set());
    selectionAnchor.current = null;
    selectionCursor.current = null;
  }, [value]);

  function emit(nextChips: Chip[], nextDraftText: string) {
    const serialized = serialize(nextChips, nextDraftText);
    lastEmitted.current = serialized;
    setChips(nextChips);
    setDraftText(nextDraftText);
    onChange(serialized);
  }

  function clearSelection() {
    if (selected.size) setSelected(new Set());
    selectionAnchor.current = null;
    selectionCursor.current = null;
  }

  function removeChips(keys: Set<string>) {
    emit(chips.filter((chip) => !keys.has(chipKey(chip))), draftText);
    if ([...keys].some((key) => selected.has(key))) {
      setSelected(new Set([...selected].filter((key) => !keys.has(key))));
    }
    if (selectionAnchor.current && keys.has(selectionAnchor.current)) selectionAnchor.current = null;
    if (selectionCursor.current && keys.has(selectionCursor.current)) selectionCursor.current = null;
  }

  function removeChip(target: Chip) {
    removeChips(new Set([chipKey(target)]));
  }

  function removeSelectedChips() {
    if (!selected.size) return;
    emit(chips.filter((chip) => !selected.has(chipKey(chip))), draftText);
    clearSelection();
  }

  function selectedChips(): Chip[] {
    return chips.filter((chip) => selected.has(chipKey(chip)));
  }

  function clipboardTextForSelection(): string | null {
    const targets = selectedChips();
    if (!targets.length) return null;
    return targets.map(formatChip).join(", ");
  }

  // Selects chips[index], or, when extending, every chip between the anchor
  // and it. The chip at `index` becomes the end arrow keys move from.
  function selectAt(index: number, extend: boolean) {
    const key = chipKey(chips[index]);
    selectionCursor.current = key;
    const anchorIndex = extend && selectionAnchor.current
      ? chips.findIndex((chip) => chipKey(chip) === selectionAnchor.current)
      : -1;
    if (anchorIndex >= 0) {
      const [from, to] = anchorIndex < index ? [anchorIndex, index] : [index, anchorIndex];
      setSelected(new Set(chips.slice(from, to + 1).map(chipKey)));
      return;
    }
    setSelected(new Set([key]));
    selectionAnchor.current = key;
  }

  function toggleChip(chip: Chip) {
    const key = chipKey(chip);
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setSelected(next);
    selectionAnchor.current = key;
    selectionCursor.current = key;
  }

  function writeClipboard(event: ReactClipboardEvent, text: string) {
    event.preventDefault();
    event.clipboardData.setData("text/plain", text);
  }

  // WebKit disables Cut and Copy, and sends no clipboard event, while the
  // input has no text selected, so the shortcuts write the clipboard here.
  // A cut removes the chips only once the write succeeds, and only if the
  // field hasn't changed in the meantime.
  function copySelectionWithShortcut(cut: boolean): boolean {
    const text = clipboardTextForSelection();
    const clipboard = typeof navigator === "undefined" ? undefined : navigator.clipboard;
    if (!text || !clipboard?.writeText) return false;
    const before = lastEmitted.current;
    const remaining = chips.filter((chip) => !selected.has(chipKey(chip)));
    const keptDraftText = draftText;
    void clipboard.writeText(text).then(() => {
      if (!cut || lastEmitted.current !== before) return;
      emit(remaining, keptDraftText);
      setSelected(new Set());
      selectionAnchor.current = null;
      selectionCursor.current = null;
    }, logBackgroundFailure(cut ? "Cut recipients" : "Copy recipients"));
    return true;
  }

  function onChipMouseDown(event: ReactMouseEvent) {
    if (disabled || (event.target as Element).closest(".recipient-chip-remove")) return;
    chipPointer.current = true;
    // A press that never becomes a click (released elsewhere) must not keep
    // suppressing the next blur or focus. Click runs before this timeout.
    window.addEventListener("mouseup", () => setTimeout(() => { chipPointer.current = false; }, 0), { once: true });
  }

  function onChipClick(chip: Chip, event: ReactMouseEvent) {
    if (disabled) return;
    event.preventDefault();
    if (event.shiftKey) selectAt(chips.indexOf(chip), true);
    else if (event.metaKey || event.ctrlKey) toggleChip(chip);
    else selectAt(chips.indexOf(chip), false);
    chipPointer.current = true;
    inputRef.current?.focus();
    chipPointer.current = false;
  }

  function inputHasTextSelection(): boolean {
    const input = inputRef.current;
    return Boolean(input && input.selectionStart !== input.selectionEnd);
  }

  // Only addresses become chips; anything else stays typed, with a hint.
  function commitDraftText() {
    const trimmed = draftText.trim();
    if (!trimmed) return;
    const { addresses, rest } = splitTypedAddresses(trimmed);
    emit(mergeTyped(chips, addresses), rest);
    setRejected(Boolean(rest));
  }

  const invalidChips = chips.filter((chip) => !isEmailAddress(chip.email));
  const selectedList = selectedChips();
  const selectionAnnouncement = selectedList.length === 1
    ? `${selectedList[0].displayName ?? selectedList[0].email} selected in ${label}`
    : selectedList.length > 1
      ? `${selectedList.length} recipients selected in ${label}`
      : "";
  const hint = rejected && draftText.trim()
    ? `“${draftText.trim()}” isn't an email address. Choose a contact or type a full address.`
    : invalidChips.length
      ? `${invalidChips.map((chip) => `“${chip.email}”`).join(", ")} ${invalidChips.length === 1 ? "isn't an email address" : "aren't email addresses"}. Remove ${invalidChips.length === 1 ? "it" : "them"} or type a full address.`
      : null;

  const visibleSuggestions = suggestions.filter(
    (contact) => !excluded.some((email) => email.toLowerCase() === contact.email.toLowerCase())
      && !chips.some((chip) => chip.email.toLowerCase() === contact.email.toLowerCase()),
  );
  const draftEmail = draftText.trim() ? parseAddress(draftText.trim()).email : "";
  const candidateEmail = draftEmail && looksLikeCompleteAddress(draftEmail) ? draftEmail : null;
  const alreadyKnown = candidateEmail
    ? visibleSuggestions.some((contact) => contact.email.toLowerCase() === candidateEmail.toLowerCase())
    : true;
  // An address that is neither saved nor in mail history never turns up from
  // `listContactSuggestions` on its own — this is the only way to pin it.
  const addCandidate: Chip | null = candidateEmail && !alreadyKnown ? toChip(draftText.trim()) : null;
  // Groups come first: typing a group's name is a deliberate choice, and
  // choosing one adds every member as a chip, so nothing downstream changes.
  const groupOptions = matchingGroups(groups, draftText);
  const contactOffset = groupOptions.length;
  const addIndex = contactOffset + visibleSuggestions.length;
  const optionCount = addIndex + (addCandidate ? 1 : 0);
  const visible = !dismissed && optionCount > 0;
  const effectiveActiveIndex = optionCount > 0 ? Math.min(Math.max(activeIndex, 0), optionCount - 1) : -1;

  function fetchSuggestions(forToken: string, thisRequest = ++requestId.current) {
    const highlightedEmail = visibleSuggestions[effectiveActiveIndex - contactOffset]?.email;
    const lookup = seeds.length || excluded.length
      ? mailClient.listContactSuggestions(account, forToken, 8, { seeds, excluded })
      : mailClient.listContactSuggestions(account, forToken, 8);
    void lookup.then((results) => {
      if (thisRequest !== requestId.current) return;
      setSuggestions(results);
      const index = results.findIndex((item) => item.email === highlightedEmail);
      setActiveIndex(index >= 0 ? contactOffset + index : 0);
    }).catch((reason) => {
      if (thisRequest !== requestId.current) return;
      setSuggestions([]);
      logBackgroundFailure("Contact suggestion lookup")(reason);
      // Keep ordinary search available if contextual evidence cannot be read.
      if (seeds.length && forToken) void mailClient.listContactSuggestions(account, forToken, 8).then((results) => {
        if (thisRequest === requestId.current) setSuggestions(results.map((item) => ({ ...item, relatedTo: [] })));
      }).catch(logBackgroundFailure("Contact suggestion fallback"));
    });
  }

  function query(nextDraftText: string) {
    if (debounce.current) clearTimeout(debounce.current);
    // Invalidate immediately, including during the next debounce and on clear.
    const thisRequest = ++requestId.current;
    const token = nextDraftText.trim();
    if ((!token && !seeds.length) || !account) {
      setSuggestions([]);
      return;
    }
    debounce.current = setTimeout(() => fetchSuggestions(token, thisRequest), 150);
  }

  function loadGroups() {
    if (groupsLoaded.current || groupsLoading.current) return;
    groupsLoading.current = true;
    void mailClient.listContactGroupRecipients().then((result) => {
      groupsLoaded.current = true;
      setGroups(result);
    }).catch(logBackgroundFailure("Contact group lookup")).finally(() => { groupsLoading.current = false; });
  }

  function selectGroup(group: ContactGroupRecipients) {
    let next = chips;
    for (const member of group.members) next = mergeChip(next, { email: member.email, displayName: member.displayName });
    const added = next.length - chips.length;
    const already = group.members.length - added;
    emit(next, "");
    setGroupNote(`Added ${added} ${added === 1 ? "person" : "people"} from ${group.name}${already ? ` · ${already} already in ${label}` : ""}`);
    setDismissed(false);
    setActiveIndex(-1);
    inputRef.current?.focus();
  }

  function select(contact: ContactSuggestion) {
    emit(mergeChip(chips, { email: contact.email, displayName: contact.displayName }), "");
    setDismissed(false);
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
    <div
      className={`compose-field recipient-field${dragOver ? " drag-over" : ""}`}
      onDragEnter={(event) => {
        if (disabled || !dragOrigin || dragOrigin.field === id) return;
        event.preventDefault();
        setDragOver(true);
      }}
      onDragOver={(event) => {
        if (disabled || !dragOrigin || dragOrigin.field === id) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(event) => {
        setDragOver(false);
        if (disabled || !dragOrigin) return;
        event.preventDefault();
        event.stopPropagation();
        const raw = event.dataTransfer.getData("application/x-threestrands-recipient");
        const origin = dragOrigin;
        dragOrigin = null;
        if (!raw || origin?.field === id) return;
        const dropped = parseDroppedChips(raw);
        if (!dropped) return;
        const next = dropped.reduce(mergeChip, chips);
        if (next !== chips) emit(next, draftText);
        origin?.remove();
      }}
    >
      {onLabelClick ? (
        <button type="button" className="recipient-field-label" aria-expanded={labelExpanded} title={labelExpanded ? "Click to hide Cc/Bcc" : "Click to show Cc/Bcc"} onClick={onLabelClick} disabled={disabled}>{label}</button>
      ) : (
        <label htmlFor={inputId}>{label}</label>
      )}
      <div className="recipient-chip-row">
        {chips.map((chip) => {
          const isSelected = selected.has(chipKey(chip));
          const invalid = invalidChips.includes(chip);
          return (
          <span
            key={chip.email}
            className={`recipient-chip${invalid ? " invalid" : ""}${isSelected ? " selected" : ""}`}
            draggable={!disabled}
            onMouseDown={onChipMouseDown}
            onClick={(event) => onChipClick(chip, event)}
            onDragStart={(event) => {
              chipPointer.current = false;
              // Dragging a selected chip carries the whole selection.
              const moving = isSelected ? selectedChips() : [chip];
              const keys = new Set(moving.map(chipKey));
              dragOrigin = { field: id, remove: () => removeChips(keys) };
              event.dataTransfer.setData("application/x-threestrands-recipient", JSON.stringify(moving));
              event.dataTransfer.setData("text/plain", moving.map(formatChip).join(", "));
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
                onClick={(event) => {
                  event.stopPropagation();
                  removeChip(chip);
                }}
              >
                <X size={ICON_SIZE.xs} />
              </button>
            )}
          </span>
          );
        })}
        <input
          {...SEARCH_INPUT_ATTRIBUTES}
          id={inputId}
          ref={inputRef}
          name={id}
          aria-label={label}
          aria-autocomplete="list"
          aria-invalid={hint ? true : undefined}
          aria-describedby={hint ? hintId : undefined}
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
            clearSelection();
            emit(chips, event.target.value);
            setRejected(false);
            setDismissed(false);
            setGroupNote(null);
            query(event.target.value);
          }}
          onFocus={() => {
            loadGroups();
            // Clicking a chip selects it; that shouldn't open suggestions.
            if (chipPointer.current) return;
            setDismissed(false);
            query(draftText);
          }}
          onBlur={() => {
            setDismissed(true);
            commitDraftText();
            if (!chipPointer.current) clearSelection();
          }}
          onCopy={(event) => {
            if (inputHasTextSelection()) return;
            const text = clipboardTextForSelection();
            if (!text) return;
            writeClipboard(event, text);
          }}
          onCut={(event) => {
            if (inputHasTextSelection()) return;
            const text = clipboardTextForSelection();
            if (!text) return;
            writeClipboard(event, text);
            removeSelectedChips();
          }}
          onPaste={(event) => {
            const raw = event.clipboardData.getData("text");
            const pasted = normalizePastedEmailAddresses(raw);
            // What the input would hold after a native paste: the pasted text
            // replaces any selected text at the caret.
            const input = event.currentTarget;
            const start = input.selectionStart ?? draftText.length;
            const before = draftText.slice(0, start);
            const after = draftText.slice(input.selectionEnd ?? start);
            const combined = `${before}${pasted}${after}`;
            // Lists always go through the custom path. So does a lone address
            // pasted as a word of its own, so a badge cut from one field pastes
            // into another as a badge right away. Anything else pastes natively.
            if (pasted === raw && !/[,;\n]/.test(pasted)) {
              const standalone = (!before || /\s$/.test(before)) && (!after || /^\s/.test(after));
              if (!standalone || !isValidRecipient(pasted)) return;
            }
            event.preventDefault();
            clearSelection();
            const { addresses, rest } = splitTypedAddresses(combined);
            emit(mergeTyped(chips, addresses), rest);
            setRejected(Boolean(rest));
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              // Sending stops at this field if its text isn't an address; say why here.
              commitDraftText();
              return;
            }
            const mod = (event.metaKey || event.ctrlKey) && !event.altKey;
            const letter = event.key.toLowerCase();
            if (mod && letter === "a" && draftText === "" && chips.length > 0) {
              event.preventDefault();
              setSelected(new Set(chips.map(chipKey)));
              selectionAnchor.current = chipKey(chips[0]);
              selectionCursor.current = chipKey(chips[chips.length - 1]);
              return;
            }
            if (mod && !event.shiftKey && (letter === "c" || letter === "x") && selected.size > 0 && !inputHasTextSelection()) {
              if (copySelectionWithShortcut(letter === "x")) {
                event.preventDefault();
                return;
              }
            }
            // Arrow keys move from the caret onto the chips and between them;
            // Shift extends the selection.
            if ((event.key === "ArrowLeft" || event.key === "ArrowRight") && !event.metaKey && !event.ctrlKey && !event.altKey
                && chips.length > 0 && !inputHasTextSelection()) {
              const cursorIndex = selected.size ? chips.findIndex((chip) => chipKey(chip) === selectionCursor.current) : -1;
              if (event.key === "ArrowLeft" && (selected.size > 0 || inputRef.current?.selectionStart === 0)) {
                event.preventDefault();
                selectAt(cursorIndex < 0 ? chips.length - 1 : Math.max(cursorIndex - 1, 0), event.shiftKey);
                return;
              }
              if (event.key === "ArrowRight" && selected.size > 0) {
                event.preventDefault();
                if (cursorIndex >= 0 && cursorIndex + 1 < chips.length) selectAt(cursorIndex + 1, event.shiftKey);
                else if (!event.shiftKey) clearSelection();
                return;
              }
            }
            if (event.key === ",") {
              event.preventDefault();
              commitDraftText();
              return;
            }
            if ((event.key === "Backspace" || event.key === "Delete") && selected.size > 0 && !inputHasTextSelection()) {
              event.preventDefault();
              removeSelectedChips();
              return;
            }
            if (event.key === "Backspace" && draftText === "" && chips.length > 0) {
              event.preventDefault();
              removeChip(chips[chips.length - 1]);
              return;
            }
            if (event.key === "Escape" && selected.size > 0) {
              event.preventDefault();
              clearSelection();
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
                {!!contact.relatedTo?.length && <small>Often emailed with {contact.relatedTo.join(", ")}</small>}
              </span>
              {/* Out of the Tab order: Tab leaves the field, which closes this list. */}
              <button
                type="button"
                tabIndex={-1}
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
    {hint && <p id={hintId} className="recipient-field-hint" role="alert">{hint}</p>}
    {groupNote && <p className="recipient-group-note" role="status">{groupNote}</p>}
    {/* Chips aren't focusable, so selecting them is announced from here. */}
    <span className="sr-only" aria-live="polite">{selectionAnnouncement}</span>
    </>
  );
}
