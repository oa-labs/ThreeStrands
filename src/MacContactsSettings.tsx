import { formatDateTime } from "./dateFormatting";
import { useEffect, useState } from "react";
import { mailClient } from "./data/client";
import type { SystemContactsStatus } from "./domain";
import { errorMessage } from "./errors";

export function MacContactsEntry({ onOpen, disabled }: { onOpen(): void; disabled: boolean }) {
  const [supported, setSupported] = useState(false);
  useEffect(() => {
    let active = true;
    if (mailClient.systemContactsStatus) void Promise.resolve(mailClient.systemContactsStatus()).then(status => { if (active) setSupported(status?.supported ?? false); }).catch(() => {});
    return () => { active = false; };
  }, []);
  return supported ? <button type="button" className="btn" disabled={disabled} onClick={onOpen}>Mac Contacts…</button> : null;
}

export function MacContactsSettings({ onChanged }: { onChanged(): void }) {
  const [status, setStatus] = useState<SystemContactsStatus | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const accept = (value: SystemContactsStatus) => { setStatus(value); setSelected(value.connection.selectedSources); };
  useEffect(() => {
    let active = true;
    void mailClient.systemContactsStatus().then(value => { if (active) accept(value); }).catch(reason => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, []);
  const run = async (work: () => Promise<SystemContactsStatus>) => {
    setBusy(true); setError("");
    try {
      accept(await work()); onChanged();
      window.dispatchEvent(new Event("threestrands:system-contacts-changed"));
    } catch (reason) {
      setError(errorMessage(reason));
      try { accept(await mailClient.systemContactsStatus()); } catch { /* Retain the last successful status. */ }
    } finally { setBusy(false); }
  };
  const permitted = status?.permission === "authorized" || status?.permission === "limited";
  return <section aria-label="Mac Contacts connection">
    <p id="mac-contacts-help">Read names, email addresses, companies, and job titles from selected address books on this Mac. Apple handles syncing with iCloud, Google, and other accounts. ThreeStrands never changes Apple Contacts.</p>
    <p>Choose only the address books you want in Contacts and recipient suggestions. Changes you save here become independent ThreeStrands profiles. The connection and downloaded contacts stay on this Mac; saved profiles follow your existing export and encrypted sync settings.</p>
    {status?.permission === "denied" || status?.permission === "restricted" ? <p>Contacts access is off. Allow ThreeStrands in System Settings → Privacy &amp; Security → Contacts, then try again.</p> : null}
    {status?.permission === "limited" ? <p>macOS has granted access to only some contacts. ThreeStrands can read only those contacts.</p> : null}
    <button type="button" className="btn" disabled={busy || !status?.supported} aria-describedby="mac-contacts-help" onClick={() => void run(() => mailClient.discoverSystemContacts())}>{permitted ? "Find address books" : "Allow Mac Contacts…"}</button>
    {permitted && status?.connection.sources.length ? <>
      <fieldset disabled={busy}><legend>Address books</legend>{status.connection.sources.map(source => <label key={source.id}><input type="checkbox" checked={selected.includes(source.id)} onChange={event => setSelected(current => event.target.checked ? [...current, source.id] : current.filter(id => id !== source.id))}/>{source.name || "Unnamed address book"}</label>)}</fieldset>
      <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void run(() => mailClient.refreshSystemContacts(selected))}>Use selected address books</button>
    </> : null}
    {status?.connection.selectedSources.length ? <>
      <p role="status">{status.contactCount} {status.contactCount === 1 ? "contact" : "contacts"} with email addresses connected.{status.connection.refreshedAt ? ` Last refreshed ${formatDateTime(new Date(status.connection.refreshedAt))}.` : ""} Refreshes automatically while ThreeStrands is running.</p>
      <button type="button" className="btn" disabled={busy || !permitted} onClick={() => void run(() => mailClient.refreshSystemContacts())}>Refresh now</button>
      <button type="button" className="btn" disabled={busy} onClick={() => void run(() => mailClient.refreshSystemContacts([]))}>Disconnect Mac Contacts</button>
      <p>Disconnecting removes downloaded contacts. Profiles you saved in ThreeStrands remain.</p>
    </> : null}
    {busy ? <p role="status">Reading Mac Contacts…</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}
