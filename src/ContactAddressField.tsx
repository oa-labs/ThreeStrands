import { useEffect, useId, useRef, useState, type ClipboardEvent, type KeyboardEvent } from "react";
import { Check, Copy, X } from "lucide-react";

const EMAIL_SEPARATOR_KEYS = new Set(["Enter", ",", ";", " "]);
const LINK_SEPARATOR_KEYS = new Set(["Enter", " "]);
const EMAIL_SEPARATORS = /[\s,;]+/;
const LINK_SEPARATORS = /\s+/;

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
 * saved contact validates addresses.
 */
export function ContactAddressField({ addresses, disabled, onChange }: {
  addresses: string[];
  disabled?: boolean;
  onChange(addresses: string[]): void;
}) {
  return <ContactMultiValueField kind="email" values={addresses} disabled={disabled} onChange={onChange} />;
}

export function ContactLinksField({ links, disabled, onChange }: {
  links: string[];
  disabled?: boolean;
  onChange(links: string[]): void;
}) {
  return <ContactMultiValueField kind="link" values={links} disabled={disabled} onChange={onChange} />;
}

function ContactMultiValueField({ kind, values, disabled, onChange }: {
  kind: "email" | "link";
  values: string[];
  disabled?: boolean;
  onChange(values: string[]): void;
}) {
  const isEmail = kind === "email";
  const label = isEmail ? "Emails" : "Links";
  const itemName = isEmail ? "address" : "link";
  const labelId = useId();
  const inputRef = useRef<HTMLInputElement>(null);
  const [draft, setDraft] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const [copyFailed, setCopyFailed] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (copiedTimer.current) clearTimeout(copiedTimer.current); }, []);

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
    const next = addAll(values, text, kind);
    setDraft("");
    if (next.length !== values.length) onChange(next);
  };
  const remove = (target: string) => {
    onChange(values.filter((value) => value !== target));
    inputRef.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
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
    <div className="contact-multi-value-field">
      <span id={labelId}>{label}</span>
      <div className="contact-multi-value-box" onClick={(event) => { if (event.target === event.currentTarget) inputRef.current?.focus(); }}>
        {values.map((value) => (
          <span key={value} className="recipient-chip contact-multi-value-chip">
            <span className="recipient-chip-label" title={value}>{value}</span>
            <button type="button" className="recipient-chip-remove contact-multi-value-copy" aria-label={copied === value ? `Copied ${value}` : `Copy ${value}`} title={`Copy ${itemName}`} onClick={() => void copy(value)}>
              {copied === value ? <Check size={12} aria-hidden="true" /> : <Copy size={12} aria-hidden="true" />}
            </button>
            <button type="button" className="recipient-chip-remove" aria-label={`Remove ${value}`} disabled={disabled} onClick={() => remove(value)}>
              <X size={12} aria-hidden="true" />
            </button>
          </span>
        ))}
        <input
          ref={inputRef}
          aria-label={isEmail ? "Email addresses" : "Links"}
          aria-describedby={labelId}
          type="text"
          inputMode={isEmail ? "email" : "url"}
          autoComplete="off"
          spellCheck={false}
          placeholder={values.length ? `Add another ${itemName}` : isEmail ? "name@example.com" : "https://example.com"}
          disabled={disabled}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => commit(draft)}
        />
      </div>
      {copyFailed ? <span className="contact-sidebar-copy-status" role="status">Could not copy {isEmail ? "email address" : "link"}</span> : null}
    </div>
  );
}
