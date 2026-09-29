import { useEffect, useId, useRef, useState, type ClipboardEvent, type KeyboardEvent } from "react";
import { Check, Copy, X } from "lucide-react";

// Typing one of these ends the address in progress and turns it into a badge.
const SEPARATOR_KEYS = new Set(["Enter", ",", ";", " "]);
const SEPARATORS = /[\s,;]+/;

function addAll(addresses: string[], text: string): string[] {
  const next = [...addresses];
  for (const token of text.split(SEPARATORS)) {
    const email = token.trim();
    if (email && !next.some((value) => value.toLocaleLowerCase() === email.toLocaleLowerCase())) next.push(email);
  }
  return next;
}

/**
 * A contact's email addresses as removable badges. Typing an address and
 * pressing Enter, comma, semicolon, or space (or leaving the field) adds it;
 * pasting a list adds each address. Each badge can copy its address. The
 * saved contact validates addresses.
 */
export function ContactAddressField({ addresses, disabled, onChange }: {
  addresses: string[];
  disabled?: boolean;
  onChange(addresses: string[]): void;
}) {
  const labelId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (copiedTimer.current) clearTimeout(copiedTimer.current); }, []);

  const copy = async (address: string) => {
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(address);
      setCopied(address);
      if (copiedTimer.current) clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(null), 1500);
    } catch {
      setCopied(null);
      setCopyFailed(true);
    }
  };

  const commit = (text: string) => {
    const next = addAll(addresses, text);
    setDraft("");
    if (next.length !== addresses.length) onChange(next);
  };
  const remove = (target: string) => {
    onChange(addresses.filter((value) => value !== target));
    inputRef.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (SEPARATOR_KEYS.has(event.key)) {
      event.preventDefault();
      commit(draft);
    } else if (event.key === "Backspace" && !draft && addresses.length) {
      event.preventDefault();
      onChange(addresses.slice(0, -1));
    }
  };
  const onPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const text = event.clipboardData.getData("text");
    if (!SEPARATORS.test(text.trim())) return;
    event.preventDefault();
    commit(`${draft} ${text}`);
  };

  return (
    <div className="contact-address-field">
      <span id={labelId}>Emails</span>
      <div className="contact-address-box" onClick={(event) => { if (event.target === event.currentTarget) inputRef.current?.focus(); }}>
        {addresses.map((address) => (
          <span key={address} className="recipient-chip contact-address-chip">
            <span className="recipient-chip-label" title={address}>{address}</span>
            <button type="button" className="recipient-chip-remove contact-address-copy" aria-label={copied === address ? `Copied ${address}` : `Copy ${address}`} title="Copy address" onClick={() => void copy(address)}>
              {copied === address ? <Check size={12} aria-hidden="true" /> : <Copy size={12} aria-hidden="true" />}
            </button>
            <button type="button" className="recipient-chip-remove" aria-label={`Remove ${address}`} disabled={disabled} onClick={() => remove(address)}>
              <X size={12} aria-hidden="true" />
            </button>
          </span>
        ))}
        <input
          ref={inputRef}
          aria-label="Email addresses"
          aria-describedby={labelId}
          type="text"
          inputMode="email"
          autoComplete="off"
          spellCheck={false}
          placeholder={addresses.length ? "Add another address" : "name@example.com"}
          disabled={disabled}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => commit(draft)}
        />
      </div>
      {copyFailed ? <span className="contact-sidebar-copy-status" role="status">Could not copy email address</span> : null}
    </div>
  );
}
