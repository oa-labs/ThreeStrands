import { useState } from "react";
import { Modal } from "./AppChrome";
import { mailClient } from "./data/client";
import type { ContactFormat, ContactImportPreview, ContactProfile } from "./domain";
import { errorMessage } from "./errors";

const name = (profile: Pick<ContactProfile, "displayName" | "addresses">) => profile.displayName || profile.addresses[0];

export function ContactManagement({ addresses, onChanged, hasUnsavedChanges = false }: { addresses: string[]; onChanged(): void; hasUnsavedChanges?: boolean }) {
  const [mode, setMode] = useState<"menu" | "import" | "export" | "suggestions" | null>(null);
  const [preview, setPreview] = useState<ContactImportPreview | null>(null);
  const [suppressed, setSuppressed] = useState<string[]>([]);
  const [email, setEmail] = useState("");
  const [format, setFormat] = useState<ContactFormat>("csv");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true); setError(null);
    try { await work(); } catch (reason) { setError(errorMessage(reason)); } finally { setBusy(false); }
  };
  const chooseImport = () => run(async () => {
    const result = await mailClient.previewContactImport();
    if (result) { setPreview(result); setMode("import"); }
  });
  const importContacts = () => run(async () => {
    if (!preview) return;
    const result = await mailClient.importContacts(preview.contacts);
    setNotice(`Imported ${result.imported} contacts. Skipped ${result.skipped + preview.skipped}.`);
    setPreview(null); setMode(null); onChanged();
  });
  const suggestions = () => run(async () => { setSuppressed(await mailClient.listContactSuppressions()); setMode("suggestions"); });
  const suppress = (address: string, value: boolean) => run(async () => {
    await mailClient.setContactSuppressed(address, value);
    setSuppressed(await mailClient.listContactSuppressions()); setEmail("");
  });
  const title = mode === "import" ? "Review contact import" : mode === "export" ? "Export saved contacts" : mode === "suggestions" ? "Never suggest these addresses" : "Manage contacts";
  return <>
    <button type="button" className="btn" onClick={() => { setError(null); setNotice(null); setMode("menu"); }}>Manage Contacts…</button>
    {notice ? <span className="contacts-status" role="status">{notice}</span> : null}
    {mode ? <Modal title={title} onClose={() => setMode(null)} dismissible={!busy} className="contact-management-dialog">
      {mode === "menu" ? <div className="contact-management-options">
        <button type="button" className="btn" disabled={busy || hasUnsavedChanges} onClick={() => void chooseImport()}>Import CSV or vCard…</button>
        <button type="button" className="btn" disabled={busy} onClick={() => setMode("export")}>Export CSV or vCard…</button>
        <button type="button" className="btn" disabled={busy} onClick={() => void suggestions()}>Manage recipient suggestions…</button>
        {hasUnsavedChanges ? <p>Save or discard profile edits before importing contacts.</p> : null}
      </div> : null}
      {mode === "import" && preview ? <>
        <p>{preview.contacts.length} contacts ready to import. Existing profiles are kept; records sharing an existing email address are skipped.</p>
        {preview.skipped ? <p>{preview.skipped} records without valid email addresses were skipped.</p> : null}
        {preview.warnings.length ? <div className="notice"><p>Review before importing:</p><ul>{preview.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></div> : null}
        <details><summary>Contacts to import</summary><ul>{preview.contacts.map((contact, index) => <li key={index}>{name(contact)} — {contact.addresses.join(", ")}</li>)}</ul></details>
        <div className="modal-actions"><button type="button" className="btn" disabled={busy} onClick={() => setMode(null)}>Cancel</button><button type="button" className="btn btn-primary" disabled={busy || !preview.contacts.length} onClick={() => void importContacts()}>Import contacts</button></div>
      </> : null}
      {mode === "export" ? <>
        <p>Export all saved contacts across accounts. Mail-derived suggestions, photos, groups, and keep-in-touch settings are excluded. Use encrypted settings export to transfer those saved details.</p>
        <p>Contact files are unencrypted. CSV exports make formula-looking values safe for spreadsheets by prefixing an apostrophe.</p>
        <label>File format<select value={format} disabled={busy} onChange={event => setFormat(event.target.value as ContactFormat)}><option value="csv">CSV</option><option value="vcard">vCard (.vcf)</option></select></label>
        <div className="modal-actions"><button type="button" className="btn" disabled={busy} onClick={() => setMode(null)}>Cancel</button><button type="button" className="btn btn-primary" disabled={busy} onClick={() => void run(async () => { if (await mailClient.exportContacts(format)) { setNotice("Saved contacts exported."); setMode(null); } })}>Export contacts</button></div>
      </> : null}
      {mode === "suggestions" ? <>
        <p>Hide an address from automatic recipient suggestions on this device, across all accounts. You can still type it or deliberately choose a group containing it. This preference survives deleting a contact and stays on this device.</p>
        {addresses.filter(address => !suppressed.includes(address.toLowerCase())).map(address => <p key={address}><button type="button" className="btn" disabled={busy} onClick={() => void suppress(address, true)}>Never suggest {address}</button></p>)}
        <form onSubmit={event => { event.preventDefault(); void suppress(email, true); }}><label>Email address<input type="email" required value={email} disabled={busy} onChange={event => setEmail(event.target.value)}/></label><button type="submit" className="btn" disabled={busy || !email.trim()}>Never suggest this address</button></form>
        {suppressed.length ? <ul>{suppressed.map(address => <li key={address}><span>{address}</span><button type="button" className="btn btn-sm" disabled={busy} onClick={() => void suppress(address, false)}>Allow suggestions for {address}</button></li>)}</ul> : <p>No addresses are hidden.</p>}
      </> : null}
      {busy ? <p role="status">Working…</p> : null}
      {error ? <p className="contacts-error" role="alert">{error}</p> : null}
    </Modal> : null}
  </>;
}

export function MergeContactsDialog({ contacts, onClose, onMerged }: { contacts: ContactProfile[]; onClose(): void; onMerged(profile: ContactProfile): void }) {
  const [targetId, setTargetId] = useState(contacts[0]?.id ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const merge = async () => {
    if (busy) return;
    setBusy(true); setError(null);
    try { const profile = await mailClient.mergeContacts(targetId, contacts.filter(contact => contact.id !== targetId).map(contact => contact.id)); onMerged(profile); }
    catch (reason) { setError(errorMessage(reason)); setBusy(false); }
  };
  return <Modal title="Merge contacts" onClose={onClose} dismissible={!busy} className="contact-management-dialog">
    <p>Choose the profile to keep. The other selected profiles will be removed. Addresses, notes, links, favorites, and group memberships are combined.</p>
    <p>The retained profile keeps its primary address, photo, and configured reminder. Missing details are filled from the other profiles; conflicting text details are added to Notes. Additional photos and reminder configurations are not retained.</p>
    <label>Profile to keep<select value={targetId} disabled={busy} onChange={event => setTargetId(event.target.value)}>{contacts.map(contact => <option key={contact.id} value={contact.id}>{name(contact)} — {contact.addresses[0]}</option>)}</select></label>
    {error ? <p className="contacts-error" role="alert">{error}</p> : null}
    <div className="modal-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button><button type="button" className="btn btn-primary" disabled={busy || contacts.length < 2} onClick={() => void merge()}>Merge {contacts.length} contacts</button></div>
  </Modal>;
}
