import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Paperclip, Send, X, Trash2 } from "lucide-react";
import { mailClient } from "./data/client";
import type { Draft, OutboxItem } from "./correspondence";

export type ComposerHandle = { flush(): Promise<Draft>; prepareExit(): Promise<void>; send(): void; attach(): void; close(): void };
export const Composer = forwardRef<ComposerHandle, { draft: Draft; onClose(): void; onQueued(item: OutboxItem): void }>(function Composer({ draft: initial, onClose, onQueued }, ref) {
  const [draft, setDraft] = useState(initial);
  const latest = useRef(initial);
  const generation = useRef(0);
  const savedGeneration = useRef(0);
  const pending = useRef<Promise<Draft> | null>(null);
  const [status, setStatus] = useState("Saved on this device");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [showCopies, setShowCopies] = useState(Boolean(initial.cc || initial.bcc));
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);

  function flush(): Promise<Draft> {
    if (timer.current) clearTimeout(timer.current);
    if (pending.current) return pending.current.then(() => generation.current === savedGeneration.current ? latest.current : flush());
    if (generation.current === savedGeneration.current) return Promise.resolve(latest.current);
    const version = generation.current;
    const snapshot = latest.current;
    setStatus("Saving…"); setError("");
    const saving = mailClient.saveDraft(snapshot).then((saved) => {
      savedGeneration.current = version;
      latest.current = { ...latest.current, revision: saved.revision, updatedAt: saved.updatedAt };
      if (mounted.current) { setDraft(latest.current); setStatus(generation.current === version ? "Saved on this device" : "Unsaved changes"); }
      return latest.current;
    }).catch((e) => {
      if (mounted.current) { setError(String(e)); setStatus("Not saved — retry before closing"); }
      throw e;
    }).finally(() => { pending.current = null; });
    pending.current = saving;
    return saving.then(() => generation.current === savedGeneration.current ? latest.current : flush());
  }
  function edit(field: "to" | "cc" | "bcc" | "subject" | "body", value: string) {
    latest.current = { ...latest.current, [field]: value }; generation.current++;
    setDraft(latest.current); setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(() => {}); }, 300);
  }
  async function run(action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError("");
    try { await action(); } catch (e) { setError(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  function close() { void run(async () => { await flush(); onClose(); }); }
  function send() { void run(async () => { const saved = await flush(); const item = await mailClient.queueDraft(saved.id, saved.revision); onQueued(item); }); }
  function attach() { void run(async () => { await flush(); const next = await mailClient.attachFiles(latest.current.id); latest.current = next; setDraft(next); }); }
  useImperativeHandle(ref, () => ({ flush, send, attach, close, prepareExit: async () => {
    if (busyRef.current) throw new Error("Finish the current composer action before closing.");
    busyRef.current = true; setBusy(true);
    try { await flush(); }
    catch (e) { busyRef.current = false; setBusy(false); throw e; }
  } }));
  useEffect(() => {
    mounted.current = true;
    const previous = document.activeElement as HTMLElement | null;
    panel.current?.querySelector<HTMLElement>(initial.mode === "new" || initial.mode === "forward" ? '[name="to"]' : "textarea")?.focus();
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (generation.current !== savedGeneration.current) { event.preventDefault(); event.returnValue = ""; }
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => { mounted.current = false; if (timer.current) clearTimeout(timer.current); window.removeEventListener("beforeunload", beforeUnload); previous?.focus(); };
  }, [initial.mode]);

  return <div className="compose-backdrop">
    <div ref={panel} className="composer" role="dialog" aria-modal="true" aria-label={initial.mode === "new" ? "New message" : initial.mode === "forward" ? "Forward message" : "Reply message"}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
        if (event.key === "Tab") {
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex="0"]') ?? []);
          const first = controls[0], last = controls.at(-1);
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <header><div><h2>{initial.mode === "new" ? "New message" : initial.mode === "forward" ? "Forward" : initial.mode === "replyAll" ? "Reply all" : "Reply"}</h2><span>From {draft.account}</span></div><button className="icon-button" aria-label="Save and close draft" onClick={close} disabled={busy}><X size={19} /></button></header>
      <div className="composer-content">
        <label className="compose-field"><span>To</span><input name="to" aria-label="To" value={draft.to} onChange={(e) => edit("to", e.target.value)} disabled={busy} placeholder="Name <email@example.com>" /></label>
        <button className="text-button" aria-expanded={showCopies} onClick={() => setShowCopies(!showCopies)}>Cc / Bcc</button>
        {showCopies && <>{(["cc", "bcc"] as const).map((field) => <label className="compose-field" key={field}><span>{field === "cc" ? "Cc" : "Bcc"}</span><input aria-label={field === "cc" ? "Cc" : "Bcc"} value={draft[field]} onChange={(e) => edit(field, e.target.value)} disabled={busy} /></label>)}</>}
        <label className="compose-field"><span>Subject</span><input aria-label="Subject" value={draft.subject} onChange={(e) => edit("subject", e.target.value)} disabled={busy} /></label>
        <textarea aria-label="Message body" value={draft.body} onChange={(e) => edit("body", e.target.value)} disabled={busy} placeholder="Write your message…" />
        {draft.attachments.length > 0 && <ul className="attachment-list">{draft.attachments.map((a) => <li key={a.id}><span>{a.name} <small>{Math.ceil(a.size / 1024)} KB · {a.ready ? "Ready" : "Download required"}</small></span>{!a.ready && <button disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.fetchAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}>Download</button>}<button aria-label={`Remove ${a.name}`} disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.removeAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}><X size={14} /></button></li>)}</ul>}
        {error && <div className="compose-error" role="alert">{error} <button onClick={() => void run(async () => { await flush(); })}>Retry save</button></div>}
      </div>
      <footer><button className="send-button" onClick={send} disabled={busy}><Send size={16} /> Send <kbd>⌘/Ctrl ↵</kbd></button><button onClick={attach} disabled={busy} aria-label="Attach files"><Paperclip size={17} /></button><span className="save-status" role="status">{status}</span><button disabled={busy} aria-label="Discard draft" onClick={() => void run(async () => { await flush(); await mailClient.discardDraft(draft.id); onClose(); })}><Trash2 size={16} /></button></footer>
      <p className="compose-note">Drafts are saved on this device. Send has a 10-second undo window.{!("__TAURI_INTERNALS__" in window) && " Browser preview: delivery and attachments are simulated."}</p>
    </div>
  </div>;
});
