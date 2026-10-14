import { useRef, useState, type KeyboardEvent } from "react";
import { Modal } from "./AppChrome";
import type { SenderRule } from "./domain";
import { isPersonalMailDomain } from "./contactContext";
import { errorMessage } from "./errors";
import type { SenderTarget } from "./SenderSections";

/**
 * Confirms muting a sender in the conversation's account: their address, or
 * everyone at an organization's domain. From then on sync marks their new
 * mail read and archives it on the server. Archiving what is already in the
 * Inbox is offered and on by default. A failed save keeps the choices.
 */
export function MuteSenderDialog({ target, inboxThreadIds, onMute, onClose }: {
  target: SenderTarget;
  /** The sender's conversations still in the Inbox. */
  inboxThreadIds: string[];
  onMute(matchKind: SenderRule["matchKind"], matchValue: string, archiveThreadIds: string[]): Promise<void>;
  onClose(): void;
}) {
  const email = target.email.toLocaleLowerCase();
  const domain = email.split("@")[1] ?? "";
  const offerDomain = Boolean(domain) && !isPersonalMailDomain(domain);
  const [matchKind, setMatchKind] = useState<SenderRule["matchKind"]>("address");
  const [archiveExisting, setArchiveExisting] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirm = useRef<HTMLButtonElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const count = inboxThreadIds.length;
  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await onMute(matchKind, matchKind === "domain" ? domain : email, archiveExisting ? inboxThreadIds : []);
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
  const who = matchKind === "domain" ? `anyone at ${domain}` : email;
  return <Modal title="Mute Sender" initialFocusRef={confirm} onClose={onClose}>
    <form ref={form} className="modal-form" onKeyDown={submitShortcut} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <p className="modal-form-hint">
        New mail from {who} will be marked read and archived as it arrives, in {target.accountId} and on your other devices.
        It stays in All Mail and search.
      </p>
      <label>Mute
        <select value={matchKind} disabled={!offerDomain} onChange={(event) => setMatchKind(event.target.value as SenderRule["matchKind"])}>
          <option value="address">Address {email}</option>
          {offerDomain ? <option value="domain">Anyone at {domain}</option> : null}
        </select>
      </label>
      {count > 0 ? (
        <label className="modal-form-check">
          <input type="checkbox" checked={archiveExisting} onChange={(event) => setArchiveExisting(event.target.checked)} />
          {count === 1 ? "Also archive the conversation already in your Inbox" : `Also archive the ${count} conversations already in your Inbox`}
        </label>
      ) : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="modal-form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button ref={confirm} type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Muting…" : "Mute Sender"}</button>
      </div>
    </form>
  </Modal>;
}
