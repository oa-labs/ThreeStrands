import { useEffect, useRef, useState } from "react";
import type { Account, SendingIdentity } from "./domain";
import { Modal } from "./AppChrome";
import { SafeMessage } from "./SafeMessage";
import { SignatureEditor } from "./SignatureEditor";
import { signatureDefaultsKey, type Signature, type SignatureDefaults, type SignatureState } from "./signatures";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";

export function SignaturesSettings({ library, accounts }: { library: SignatureState; accounts: Account[] }) {
  const [editor, setEditor] = useState<Signature | "new" | null>(null);
  const [preview, setPreview] = useState<Signature | null>(null);
  const [defaultsAccount, setDefaultsAccount] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Signature | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ordered = [...library.signatures].sort((a, b) => a.name.localeCompare(b.name));
  function signatureName(id: string | null | undefined) { return library.signatures.find((s) => s.id === id)?.name ?? "No signature"; }
  return <section className="settings-section accounts-manager" aria-label="Signatures">
    <div className="accounts-manager-header"><div><h3>Signatures</h3><p>Choose a sign-off for each sending address. Replies and forwards can use a shorter signature.</p></div><button className="btn btn-primary" type="button" onClick={() => setEditor("new")}>Add Signature</button></div>
    {ordered.length ? <ul className="accounts-list">{ordered.map((signature) => <li className="account-card" key={signature.id}>
      <div className="account-card-row"><div className="account-card-identity"><strong>{signature.name}</strong><span className="account-card-email">{signature.text.replace(/\s+/g, " ") || "Image signature"}</span></div></div>
      <div className="account-card-controls"><span className="accounts-list-actions">
        <button className="btn btn-sm" type="button" onClick={() => setPreview(signature)}>Preview</button>
        <button className="btn btn-sm" type="button" onClick={() => setEditor(signature)}>Edit</button>
        <button className="btn btn-sm" type="button" onClick={() => setEditor({ ...signature, id: crypto.randomUUID(), name: `${signature.name} Copy` })}>Duplicate</button>
        <button className="btn btn-sm btn-danger" type="button" onClick={() => setDeleteTarget(signature)}>Delete</button>
      </span></div>
    </li>)}</ul> : <div className="accounts-empty"><strong>No signatures yet</strong><p>Add a signature, then choose when to use it below.</p></div>}
    <h3>Default Signatures</h3>
    {accounts.length ? <ul className="accounts-list">{accounts.map((account) => {
      const defaults = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account.email));
      return <li className="account-card" key={account.email}>
        <div className="account-card-row"><div className="account-card-identity"><strong>{account.nickname || account.email}</strong><span className="account-card-email">New: {signatureName(defaults?.newSignatureId)} · Replies and forwards: {signatureName(defaults?.replySignatureId)}</span></div></div>
        <div className="account-card-controls"><button className="btn btn-sm" type="button" aria-label={`Edit Signature Defaults for ${account.email}`} onClick={() => setDefaultsAccount(account.email)}>Edit Defaults…</button></div>
      </li>;
    })}</ul> : <p className="settings-hint">Connect a mail account to choose default signatures.</p>}
    {editor ? <SignatureEditor key={editor === "new" ? "new" : editor.id} signature={editor === "new" ? null : editor} onSave={library.save} onClose={() => setEditor(null)} /> : null}
    {preview ? <Modal title="Signature Preview" onClose={() => setPreview(null)} className="signature-dialog"><div className="signature-form"><SafeMessage html={preview.html} text={preview.text} loadImages={false} foldQuotes={false} /><h3>Plain Text</h3><pre className="signature-plain-preview">{preview.text}</pre><div className="signature-actions"><button type="button" className="btn" onClick={() => setPreview(null)}>Done</button></div></div></Modal> : null}
    {defaultsAccount ? <SignatureDefaultsDialog key={defaultsAccount} account={defaultsAccount} library={library} onClose={() => setDefaultsAccount(null)} /> : null}
    {deleteTarget ? <Modal title="Delete Signature" onClose={() => { if (!busy) setDeleteTarget(null); }}><div className="signature-form"><p>Delete “{deleteTarget.name}”? Addresses using it will have no signature. Existing drafts and queued messages keep their signatures.</p>{error ? <p role="alert" className="form-error">{error}</p> : null}<div className="signature-actions"><button className="btn" type="button" disabled={busy} onClick={() => setDeleteTarget(null)}>Cancel</button><button className="btn btn-danger" type="button" disabled={busy} onClick={() => {
      setBusy(true); setError(""); void library.remove(deleteTarget.id).then(() => setDeleteTarget(null)).catch((reason) => setError(errorMessage(reason))).finally(() => setBusy(false));
    }}>Delete</button></div></div></Modal> : null}
  </section>;
}

export function SignatureDefaultsDialog({ account, library, onClose }: { account: string; library: SignatureState; onClose(): void }) {
  const addressInput = useRef<HTMLSelectElement>(null);
  const [identities, setIdentities] = useState<SendingIdentity[]>([]);
  const [address, setAddress] = useState("");
  const base = library.defaults.find((d) => signatureDefaultsKey(d.account, d.address) === signatureDefaultsKey(account));
  const [newId, setNewId] = useState<string | null>(base?.newSignatureId ?? null);
  const [replyId, setReplyId] = useState<string | null>(base?.replySignatureId ?? null);
  const [inherit, setInherit] = useState(false);
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
    <label className="snippet-field"><span>Sending Address</span><select ref={addressInput} autoFocus value={address} disabled={busy} onChange={(event) => chooseAddress(event.target.value)}><option value="">Account Defaults</option>{identities.filter((i) => i.address.toLowerCase() !== account.toLowerCase()).map((i) => <option value={i.address} key={i.address}>{i.address}</option>)}</select></label>
    {address ? <label className="settings-checkbox"><input type="checkbox" checked={inherit} disabled={busy} onChange={(event) => setInherit(event.target.checked)} /><span>Use Account Defaults</span></label> : null}
    <label className="snippet-field"><span>New Messages</span><select value={(inherit ? base?.newSignatureId : newId) ?? ""} disabled={busy || inherit} onChange={(event) => setNewId(event.target.value || null)}>{options(newId)}</select></label>
    <label className="snippet-field"><span>Replies and Forwards</span><select value={(inherit ? base?.replySignatureId : replyId) ?? ""} disabled={busy || inherit} onChange={(event) => setReplyId(event.target.value || null)}>{options(replyId)}</select></label>
    {error ? <p className="form-error" role="alert">{error}</p> : null}
    <div className="signature-actions"><button className="btn" type="button" disabled={busy} onClick={onClose}>Cancel</button><button className="btn btn-primary" type="submit" disabled={busy}>Save</button></div>
  </form></Modal>;
}
