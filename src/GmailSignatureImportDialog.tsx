import { useEffect, useRef, useState } from "react";
import type { Account } from "./domain";
import { Modal } from "./AppChrome";
import { SignaturePreview } from "./SignaturePreview";
import { mailClient } from "./data/client";
import { errorMessage } from "./errors";
import { SIGNATURE_LIMITS } from "./emailRenderingPolicy";
import { validateSignature, type GmailSignature } from "./signatures";
import { prepareSignatureImport, remoteImageSources, signatureImportContent } from "./signatureImport";

function importedName(address: string) {
  return Array.from(`Gmail — ${address}`).slice(0, SIGNATURE_LIMITS.maxNameLength).join("");
}

export function GmailSignatureImportDialog({ accounts, onImported, onClose }: {
  accounts: Account[]; onImported(): Promise<void>; onClose(): void;
}) {
  const accountInput = useRef<HTMLSelectElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const embeddedStatusRef = useRef<HTMLParagraphElement>(null);
  const [account, setAccount] = useState(accounts[0]?.email ?? "");
  const [sources, setSources] = useState<GmailSignature[]>([]);
  const [address, setAddress] = useState("");
  const [name, setName] = useState("");
  const [id, setId] = useState(() => crypto.randomUUID());
  const [embedded, setEmbedded] = useState<string | null>(null);
  const [useAsDefault, setUseAsDefault] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const selected = sources.find((source) => source.address === address);
  const content = signatureImportContent(embedded ?? selected?.html ?? "");
  const remote = remoteImageSources(embedded ?? selected?.html ?? "");

  useEffect(() => {
    let active = true;
    void mailClient.listGmailSignatures(account).then((list) => {
      if (!active) return;
      setSources(list);
      const first = list.find((source) => source.address.toLowerCase() === account.toLowerCase()) ?? list[0];
      setAddress(first?.address ?? ""); setName(first ? importedName(first.address) : "");
    }).catch((reason) => { if (active) setError(errorMessage(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [account, retry]);

  useEffect(() => {
    // Reveal the result of an explicit action without moving keyboard focus.
    if (error) errorRef.current?.scrollIntoView?.({ block: "nearest" });
    else if (embedded !== null) embeddedStatusRef.current?.scrollIntoView?.({ block: "nearest" });
  }, [error, embedded]);

  function reload(nextAccount: string) {
    setAccount(nextAccount); setSources([]); setAddress(""); setName("");
    setEmbedded(null); setId(crypto.randomUUID()); setError(""); setLoading(true);
    setRetry((value) => value + 1);
  }
  async function embedImages() {
    if (busy || !selected) return;
    setBusy(true); setError("");
    try { setEmbedded((await prepareSignatureImport(selected.html, true)).html); }
    catch (reason) { setError(errorMessage(reason)); }
    finally { setBusy(false); }
  }
  async function save() {
    if (busy || loading || !selected) return;
    setError("");
    try {
      const signature = { id, name: name.trim(), ...content };
      validateSignature(signature);
      setBusy(true);
      await mailClient.importGmailSignature(account, signature, useAsDefault);
      await onImported();
      onClose();
    } catch (reason) { setError(errorMessage(reason)); setBusy(false); }
  }

  return <Modal title="Import from Gmail" className="signature-dialog" initialFocusRef={accountInput} onClose={() => { if (!busy) onClose(); }}>
    <form className="signature-form" onSubmit={(event) => { event.preventDefault(); void save(); }} onKeyDown={(event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void save(); }
    }}>
      <p>Import a copy of a Gmail signature for new messages, then edit it in ThreeStrands.</p>
      <label className="snippet-field"><span>Gmail Account</span><select ref={accountInput} value={account} disabled={busy} onChange={(event) => reload(event.target.value)}>
        {accounts.map((item) => <option value={item.email} key={item.email}>{item.nickname ? `${item.nickname} (${item.email})` : item.email}</option>)}
      </select></label>
      {loading ? <p role="status">Loading Gmail signatures…</p> : !sources.length ? <>{!error ? <p>No signatures were found. Add a default signature for new messages in Gmail, then try again.</p> : null}<button className="btn" type="button" onClick={() => reload(account)}>Try Again</button></> : null}
      {selected ? <>
        <label className="snippet-field"><span>Sending Address</span><select value={address} disabled={busy || loading} onChange={(event) => {
          setAddress(event.target.value); setName(importedName(event.target.value)); setId(crypto.randomUUID()); setEmbedded(null); setError("");
        }}>{sources.map((source) => <option value={source.address} key={source.address}>{source.address}</option>)}</select></label>
        <label className="snippet-field"><span>Name</span><input value={name} disabled={busy} onChange={(event) => setName(event.target.value)} /></label>
        {error ? <p ref={errorRef} className="form-error" role="alert">{error}</p> : null}
        {embedded !== null ? <p ref={embeddedStatusRef} className="settings-hint" role="status">Images are embedded in the preview and will be included in the imported signature.</p> : null}
        {remote.length ? <><p className="settings-hint">Remote images are omitted from this preview. Embed them to include them in your signature.</p><button type="button" className="btn" disabled={busy} onClick={() => void embedImages()}>{busy ? "Working…" : "Embed Remote Images"}</button></> : null}
        <section aria-label="Signature Preview"><h3>Preview</h3><SignaturePreview html={content.html} text={content.text} /></section>
        <section aria-label="Plain Text Preview"><h3>Plain Text</h3><pre className="signature-plain-preview">{content.text}</pre></section>
        <label className="settings-checkbox"><input type="checkbox" checked={useAsDefault} disabled={busy} onChange={(event) => setUseAsDefault(event.target.checked)} /><span>Use as Account Default for New Messages</span></label>
        <p className="settings-hint">Applies to {account}. Your reply and forward defaults stay as they are.</p>
      </> : null}
      {error && !selected ? <p ref={errorRef} className="form-error" role="alert">{error}</p> : null}
      <div className="signature-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button><button type="submit" className="btn btn-primary" disabled={busy || loading || !selected}>{busy ? "Importing…" : "Import"}</button></div>
    </form>
  </Modal>;
}
