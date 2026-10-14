import { useRef, useState, type KeyboardEvent } from "react";
import { Modal } from "./AppChrome";
import type { SplitInboxMatchKind } from "./domain";
import { isPersonalMailDomain } from "./contactContext";
import { errorMessage } from "./errors";
import type { SenderTarget } from "./SenderSections";
import { SEARCH_INPUT_ATTRIBUTES } from "./searchInputAttributes";

/**
 * Creates a split inbox for one sender from the context panel: their address,
 * or everyone at their domain when it is an organization's rather than a
 * personal mail provider's. The split belongs to the conversation's account.
 * A failed save keeps what was entered.
 */
export function SenderSplitDialog({ target, onCreate, onClose }: {
  target: SenderTarget;
  onCreate(name: string, matchKind: SplitInboxMatchKind, matchValue: string, accountId: string): Promise<void>;
  onClose(): void;
}) {
  const email = target.email.toLocaleLowerCase();
  const domain = email.split("@")[1] ?? "";
  const offerDomain = Boolean(domain) && !isPersonalMailDomain(domain);
  const [name, setName] = useState(target.name && target.name !== target.email ? target.name : email);
  const [matchKind, setMatchKind] = useState<Extract<SplitInboxMatchKind, "pattern" | "domain">>("pattern");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const submit = async () => {
    if (!name.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onCreate(name.trim(), matchKind, matchKind === "domain" ? domain : email, target.accountId);
    } catch (reason: unknown) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  const submitShortcut = (event: KeyboardEvent<HTMLFormElement>) => {
    if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
      event.preventDefault();
      form.current?.requestSubmit();
    }
  };
  return <Modal title="New Split" initialFocusRef={input} onClose={onClose}>
    <form ref={form} className="modal-form" onKeyDown={submitShortcut} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <p className="modal-form-hint">Future and current mail that matches moves out of the Inbox into its own tab.</p>
      <label>Name
        <input {...SEARCH_INPUT_ATTRIBUTES} ref={input} value={name} onChange={(event) => setName(event.target.value)} onFocus={(event) => event.currentTarget.select()} />
      </label>
      <label>Match
        <select value={matchKind} disabled={!offerDomain} onChange={(event) => setMatchKind(event.target.value as "pattern" | "domain")}>
          <option value="pattern">Address {email}</option>
          {offerDomain ? <option value="domain">Anyone at {domain}</option> : null}
        </select>
      </label>
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={!name.trim() || busy}>{busy ? "Creating…" : "Create Split"}</button>
      </div>
    </form>
  </Modal>;
}
