import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Account, SendingIdentity } from "./domain";
import { Modal } from "./AppChrome";
import { SignaturePreview } from "./SignaturePreview";
import { SignatureEditor } from "./SignatureEditor";
import { GmailSignatureImportDialog } from "./GmailSignatureImportDialog";
import { signatureDefaultsKey, type Signature, type SignatureDefaults, type SignatureState } from "./signatures";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";

type DefaultsTarget = { account: string; address: string };

/** Where a signature is chosen, phrased for people: "Work (new messages)". */
export function signatureUsage(signatureId: string, defaults: SignatureDefaults[], accounts: Account[]): string[] {
  return defaults.flatMap((entry) => {
    const isNew = entry.newSignatureId === signatureId, isReply = entry.replySignatureId === signatureId;
    if (!isNew && !isReply) return [];
    const use = isNew && isReply ? "all messages" : isNew ? "new messages" : "replies and forwards";
    const account = accounts.find((a) => signatureDefaultsKey(a.email) === signatureDefaultsKey(entry.account));
    const name = entry.address ? entry.address : account?.nickname || account?.email || entry.account;
    return [`${name} (${use})`];
  });
}

export function SignaturesSettings({ library, accounts }: { library: SignatureState; accounts: Account[] }) {
  const [editor, setEditor] = useState<Signature | "new" | null>(null);
  const [gmailImport, setGmailImport] = useState(false);
  const gmailAccounts = accounts.filter((account) => account.provider === "gmail" && account.onThisDevice);
  const [preview, setPreview] = useState<Signature | null>(null);
  const [defaultsTarget, setDefaultsTarget] = useState<DefaultsTarget | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Signature | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ordered = [...library.signatures].sort((a, b) => a.name.localeCompare(b.name));
  function signatureChoice(id: string | null | undefined) {
    const name = library.signatures.find((s) => s.id === id)?.name;
    return name ? <span className="signature-default-value">{name}</span> : <span className="signature-default-value signature-default-none">None</span>;
  }
  function defaultsRow(key: string, label: ReactNode, defaults: SignatureDefaults | undefined, edit: ReactNode, alias = false) {
    return <li className={alias ? "signature-default-row signature-default-alias" : "signature-default-row"} key={key}>
      <div className="signature-default-identity">{label}</div>
      <dl className="signature-default-values">
        <div><dt>New messages</dt><dd>{signatureChoice(defaults?.newSignatureId)}</dd></div>
        <div><dt>Replies and forwards</dt><dd>{signatureChoice(defaults?.replySignatureId)}</dd></div>
      </dl>
      {edit}
    </li>;
  }
  const deleteUsage = deleteTarget ? signatureUsage(deleteTarget.id, library.defaults, accounts) : [];
  return <section className="settings-section accounts-manager signatures-settings" aria-label="Signatures">
    <div className="signatures-settings-group">
      <div className="accounts-manager-header"><div><h3>Default Signatures</h3><p className="settings-hint">Choose a signature for each sending address. Replies and forwards can use a shorter one.</p></div></div>
      {accounts.length ? <ul className="signature-defaults-list">{accounts.flatMap((account) => {
        const defaults = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account.email));
        const aliases = library.defaults.filter((d) => d.address && signatureDefaultsKey(d.account) === signatureDefaultsKey(account.email)).sort((a, b) => a.address.localeCompare(b.address));
        const name = account.nickname || account.email;
        return [
          defaultsRow(account.email, <><strong>{name}</strong>{name !== account.email ? <span className="signature-default-email">{account.email}</span> : null}</>, defaults,
            <button className="btn btn-sm" type="button" aria-label={`Edit Signature Defaults for ${account.email}`} onClick={() => setDefaultsTarget({ account: account.email, address: "" })}>Edit…</button>),
          ...aliases.map((alias) => defaultsRow(alias.id, <><span className="signature-default-email">{alias.address}</span><span className="signature-default-note">Overrides {name}</span></>, alias,
            <button className="btn btn-sm btn-ghost" type="button" aria-label={`Edit Override for ${alias.address}`} onClick={() => setDefaultsTarget({ account: account.email, address: alias.address })}>Edit…</button>, true)),
        ];
      })}</ul> : <p className="settings-hint">Connect a mail account to choose default signatures.</p>}
    </div>
    <div className="signatures-settings-group">
      <div className="accounts-manager-header"><div><h3>Signatures</h3></div><span className="accounts-list-actions"><button className="btn" type="button" disabled={!gmailAccounts.length} title={gmailAccounts.length ? undefined : "Connect a Gmail account on this device to import its signatures."} onClick={() => setGmailImport(true)}>Import from Gmail…</button><button className="btn btn-primary" type="button" onClick={() => setEditor("new")}>Add Signature</button></span></div>
      {ordered.length ? <ul className="signature-cards">{ordered.map((signature) => {
        const usage = signatureUsage(signature.id, library.defaults, accounts);
        return <li className="signature-card" key={signature.id}>
          <div className="signature-card-heading"><strong>{signature.name}</strong>
            <span className="signature-card-actions">
              <button className="btn btn-sm" type="button" onClick={() => setEditor(signature)}>Edit</button>
              <button className="btn btn-sm btn-ghost" type="button" onClick={() => setPreview(signature)}>Preview</button>
              <button className="btn btn-sm btn-ghost" type="button" onClick={() => setEditor({ ...signature, id: crypto.randomUUID(), name: `${signature.name} Copy` })}>Duplicate</button>
              <button className="btn btn-sm btn-ghost signature-card-delete" type="button" onClick={() => setDeleteTarget(signature)}>Delete</button>
            </span>
          </div>
          <p className="signature-card-text">{signature.text.trim() || "Image signature"}</p>
          <p className="signature-card-usage">{usage.length ? `Used by ${usage.join(", ")}` : "Not used by any address"}</p>
        </li>;
      })}</ul> : <div className="accounts-empty"><strong>No signatures yet</strong><p>Add a signature, then choose where to use it under Default Signatures.</p></div>}
    </div>
    {editor ? <SignatureEditor key={editor === "new" ? "new" : editor.id} signature={editor === "new" ? null : editor} onSave={library.save} onClose={() => setEditor(null)} /> : null}
    {gmailImport ? <GmailSignatureImportDialog accounts={gmailAccounts} onImported={library.refresh} onClose={() => setGmailImport(false)} /> : null}
    {preview ? <Modal title="Signature Preview" onClose={() => setPreview(null)} className="signature-dialog"><div className="signature-form"><SignaturePreview html={preview.html} text={preview.text} /><h3>Plain Text</h3><pre className="signature-plain-preview">{preview.text}</pre><div className="signature-actions"><button type="button" className="btn" onClick={() => setPreview(null)}>Done</button></div></div></Modal> : null}
    {defaultsTarget ? <SignatureDefaultsDialog key={signatureDefaultsKey(defaultsTarget.account, defaultsTarget.address)} account={defaultsTarget.account} address={defaultsTarget.address} library={library} onClose={() => setDefaultsTarget(null)} /> : null}
    {deleteTarget ? <Modal title="Delete Signature" onClose={() => { if (!busy) setDeleteTarget(null); }}><div className="signature-form"><p>Delete “{deleteTarget.name}”? {deleteUsage.length ? `${deleteUsage.join(", ")} will have no signature.` : "No address uses it."} Existing drafts and queued messages keep their signatures.</p>{error ? <p role="alert" className="form-error">{error}</p> : null}<div className="signature-actions"><button className="btn" type="button" disabled={busy} onClick={() => setDeleteTarget(null)}>Cancel</button><button className="btn btn-danger" type="button" disabled={busy} onClick={() => {
      setBusy(true); setError(""); void library.remove(deleteTarget.id).then(() => setDeleteTarget(null)).catch((reason) => setError(errorMessage(reason))).finally(() => setBusy(false));
    }}>Delete</button></div></div></Modal> : null}
  </section>;
}

/** `address` opens the dialog on that sending address instead of the account defaults. */
export function SignatureDefaultsDialog({ account, address: initialAddress = "", library, onClose }: { account: string; address?: string; library: SignatureState; onClose(): void }) {
  const addressInput = useRef<HTMLSelectElement>(null);
  const [identities, setIdentities] = useState<SendingIdentity[]>([]);
  const [address, setAddress] = useState(initialAddress);
  const base = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account));
  const initial = initialAddress ? library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account, initialAddress)) : base;
  const [newId, setNewId] = useState<string | null>((initial ?? base)?.newSignatureId ?? null);
  const [replyId, setReplyId] = useState<string | null>((initial ?? base)?.replySignatureId ?? null);
  const [inherit, setInherit] = useState(Boolean(initialAddress && !initial));
  // Saved overrides stay selectable even before (or without) the provider listing the alias.
  const listed = identities.map((i) => i.address);
  const addresses = [...listed, ...(initialAddress && !listed.some((a) => a.toLowerCase() === initialAddress.toLowerCase()) ? [initialAddress] : [])].filter((a) => a.toLowerCase() !== account.toLowerCase());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void mailClient.listIdentities(account).then((list) => { if (active) setIdentities(list); }).catch((reason) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [account]);
  function chooseAddress(value: string) {
    setAddress(value);
    const defaults = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account, value));
    setInherit(Boolean(value && !defaults));
    setNewId(defaults ? defaults.newSignatureId : (value ? base?.newSignatureId : null) ?? null);
    setReplyId(defaults ? defaults.replySignatureId : (value ? base?.replySignatureId : null) ?? null);
  }
  async function save() {
    if (busy) return;
    setBusy(true); setError("");
    try {
      if (address && inherit) await library.inheritDefaults(account, address);
      else {
        const defaults: SignatureDefaults = { id: signatureDefaultsKey(account, address), account, address, newSignatureId: newId, replySignatureId: replyId };
        await library.saveDefaults(defaults);
      }
      onClose();
    } catch (reason) { setError(errorMessage(reason)); setBusy(false); }
  }
  function options(id: string | null) {
    return <><option value="">No Signature</option>{id && !library.signatures.some((s) => s.id === id) ? <option value={id}>Unavailable — No Signature</option> : null}{library.signatures.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}</>;
  }
  return <Modal title="Default Signatures" initialFocusRef={addressInput} onClose={() => { if (!busy) onClose(); }}><form className="signature-form" onSubmit={(event) => { event.preventDefault(); void save(); }} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void save(); } }}>
    <p>{account}</p>
    <label className="snippet-field"><span>Sending Address</span><select ref={addressInput} autoFocus value={address} disabled={busy} onChange={(event) => chooseAddress(event.target.value)}><option value="">Account Defaults</option>{addresses.map((a) => <option value={a} key={a}>{a}</option>)}</select></label>
    {address ? <label className="settings-checkbox"><input type="checkbox" checked={inherit} disabled={busy} onChange={(event) => setInherit(event.target.checked)} /><span>Use Account Defaults</span></label> : null}
    <label className="snippet-field"><span>New Messages</span><select value={(inherit ? base?.newSignatureId : newId) ?? ""} disabled={busy || inherit} onChange={(event) => setNewId(event.target.value || null)}>{options(newId)}</select></label>
    <label className="snippet-field"><span>Replies and Forwards</span><select value={(inherit ? base?.replySignatureId : replyId) ?? ""} disabled={busy || inherit} onChange={(event) => setReplyId(event.target.value || null)}>{options(replyId)}</select></label>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    <div className="signature-actions"><button className="btn" type="button" disabled={busy} onClick={onClose}>Cancel</button><button className="btn btn-primary" type="submit" disabled={busy}>Save</button></div>
  </form></Modal>;
}
