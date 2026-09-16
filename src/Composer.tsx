import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Paperclip, Send, X, Trash2 } from "lucide-react";
import { mailClient } from "./data/client";
import type { Draft, OutboxItem } from "./correspondence";
import type { Account } from "./domain";
import { RecipientField } from "./RecipientField";
import {
  applyFormattingShortcut,
  formattingShortcutFor,
  linkifyPlainText,
  plainTextToHtml,
  sanitizeComposeHtml,
  serializeComposeHtml,
} from "./richText";
import { useEscapeDismiss } from "./useEscapeDismiss";

export type ComposerHandle = { flush(): Promise<Draft>; prepareExit(): Promise<void>; send(afterQueued?: () => void): void; attach(): void; close(): void };

export const Composer = forwardRef<ComposerHandle, { draft: Draft; accounts: Account[]; onClose(): void; onQueued(item: OutboxItem): void }>(function Composer({ draft: initial, accounts, onClose, onQueued }, ref) {
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
  const bodyEditor = useRef<HTMLDivElement>(null);
  const initialBodyHtml = useRef(sanitizeComposeHtml(initial.bodyHtml || plainTextToHtml(initial.body)));
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
  function editBody(editor: HTMLElement) {
    const html = serializeComposeHtml(editor);
    const textOnly = editor.cloneNode(true) as HTMLElement;
    textOnly.querySelectorAll("[data-compose-image-remove], [data-compose-image-resize]").forEach((control) => control.remove());
    latest.current = {
      ...latest.current,
      body: textOnly.innerText,
      bodyHtml: html,
    };
    generation.current++;
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
  function send(afterQueued?: () => void) {
    void run(async () => {
      const saved = await flush();
      const item = await mailClient.queueDraft(saved.id, saved.revision);
      onQueued(item);
      afterQueued?.();
    });
  }
  function attach() { void run(async () => { await flush(); const next = await mailClient.attachFiles(latest.current.id); latest.current = next; setDraft(next); }); }
  function changeAccount(email: string) {
    if (email === latest.current.account) return;
    void run(async () => { await flush(); const next = await mailClient.setDraftAccount(latest.current.id, email); latest.current = next; setDraft(next); });
  }
  function decorateImage(image: HTMLImageElement, attachmentId?: string) {
    if (image.closest("[data-compose-image]")) return;
    const wrapper = document.createElement("span");
    wrapper.className = "compose-image";
    wrapper.dataset.composeImage = "true";
    if (attachmentId) wrapper.dataset.attachmentId = attachmentId;
    wrapper.contentEditable = "false";
    if (image.width) wrapper.style.width = `${image.width}px`;
    image.before(wrapper);
    wrapper.append(image);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "compose-image-remove";
    remove.dataset.composeImageRemove = "true";
    remove.setAttribute("aria-label", "Remove pasted image");
    remove.title = "Remove image";
    remove.textContent = "×";
    wrapper.append(remove);

    const resize = document.createElement("span");
    resize.className = "compose-image-resize";
    resize.dataset.composeImageResize = "true";
    resize.setAttribute("role", "slider");
    resize.setAttribute("aria-label", "Resize pasted image");
    resize.setAttribute("aria-valuemin", "80");
    resize.setAttribute("aria-valuemax", "2000");
    resize.setAttribute("aria-valuenow", String(image.width || 320));
    resize.tabIndex = 0;
    resize.title = "Drag to resize";
    wrapper.append(resize);
  }
  function insertPastedImage(file: File, range: Range | null) {
    const reader = new FileReader();
    reader.onerror = () => setError(`Could not paste ${file.name || "image"}.`);
    reader.onload = () => {
      if (!mounted.current || typeof reader.result !== "string" || !bodyEditor.current) return;
      const preview = reader.result;
      const data = preview.slice(preview.indexOf(",") + 1);
      void run(async () => {
        await flush();
        const next = await mailClient.attachInlineImage(latest.current.id, file.name || "pasted-image", file.type, data);
        const attachment = next.attachments.find((candidate) => candidate.inline && !latest.current.attachments.some((existing) => existing.id === candidate.id));
        if (!attachment?.contentId || !bodyEditor.current) throw new Error("Could not prepare the pasted image");
        latest.current = next; setDraft(next);

        const image = document.createElement("img");
        image.src = preview;
        image.dataset.composeSource = `cid:${attachment.contentId}`;
        image.alt = file.name || "Pasted image";
        decorateImage(image, attachment.id);
        const wrapper = image.closest<HTMLElement>("[data-compose-image]")!;
        const insertion = range && bodyEditor.current.contains(range.commonAncestorContainer) ? range : document.createRange();
        if (!range || !bodyEditor.current.contains(range.commonAncestorContainer)) insertion.selectNodeContents(bodyEditor.current);
        insertion.collapse(false);
        insertion.deleteContents();
        insertion.insertNode(wrapper);
        const spacer = document.createTextNode("\u00a0");
        wrapper.after(spacer);
        const caret = document.createRange();
        caret.setStartAfter(spacer); caret.collapse(true);
        window.getSelection()?.removeAllRanges(); window.getSelection()?.addRange(caret);
        image.onload = () => {
          const available = bodyEditor.current?.clientWidth ?? image.naturalWidth;
          const width = Math.max(80, Math.min(image.naturalWidth, available));
          if (width) { image.setAttribute("width", String(Math.round(width))); wrapper.style.width = `${Math.round(width)}px`; }
          editBody(bodyEditor.current!);
        };
        editBody(bodyEditor.current);
      });
    };
    reader.readAsDataURL(file);
  }
  useImperativeHandle(ref, () => ({ flush, send, attach, close, prepareExit: async () => {
    if (busyRef.current) throw new Error("Finish the current composer action before closing.");
    busyRef.current = true; setBusy(true);
    try { await flush(); }
    catch (e) { busyRef.current = false; setBusy(false); throw e; }
  } }));
  useEffect(() => {
    mounted.current = true;
    const previous = document.activeElement as HTMLElement | null;
    bodyEditor.current?.querySelectorAll<HTMLImageElement>("img").forEach((image) => {
      const source = image.getAttribute("src") ?? "";
      const attachment = source.startsWith("cid:")
        ? initial.attachments.find((candidate) => candidate.inline && candidate.contentId === source.slice(4))
        : undefined;
      if (attachment) {
        image.dataset.composeSource = source;
        void mailClient.readInlineImage(initial.id, attachment.id).then((preview) => { image.src = preview; }).catch((reason) => setError(String(reason)));
      }
      decorateImage(image, attachment?.id);
    });
    panel.current?.querySelector<HTMLElement>(initial.mode === "new" || initial.mode === "forward" ? '[name="to"]' : '[contenteditable="true"]')?.focus();
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (generation.current !== savedGeneration.current) { event.preventDefault(); event.returnValue = ""; }
    };
    window.addEventListener("beforeunload", beforeUnload);
    if (initial.mode === "forward" && initial.attachments.some((attachment) => !attachment.ready)) {
      void run(async () => {
        let next = await flush();
        for (const attachment of next.attachments.filter((candidate) => !candidate.ready)) {
          next = await mailClient.fetchAttachment(next.id, attachment.id);
          latest.current = next;
          if (mounted.current) setDraft(next);
        }
      });
    }
    return () => { mounted.current = false; if (timer.current) clearTimeout(timer.current); window.removeEventListener("beforeunload", beforeUnload); previous?.focus(); };
  }, [initial.mode]);
  useEscapeDismiss(close);
  return <div ref={panel} className="composer composer-inline" role="dialog" aria-label={initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward message" : "Reply message"}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Tab") {
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), [contenteditable="true"], [tabindex="0"]') ?? []);
          const first = controls[0], last = controls.at(-1);
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <header className="composer-header"><div><h2>{initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward" : initial.mode === "replyAll" ? "Reply all" : "Reply"}</h2>{initial.mode === "new" && accounts.length > 1 ? (
        <label className="compose-from"><span>From</span><select aria-label="Send from" value={draft.account} disabled={busy} onChange={(e) => changeAccount(e.target.value)}>
          {accounts.map((a) => <option key={a.email} value={a.email}>{a.email}</option>)}
        </select></label>
      ) : <span>From {draft.account}</span>}</div><button className="icon-button" aria-label="Save and close draft" onClick={close} disabled={busy}><X size={19} /></button></header>
      <div className="composer-content">
        <RecipientField id="to" label="To" value={draft.to} account={draft.account} disabled={busy} onChange={(value) => edit("to", value)} />
        <button className="text-button" aria-expanded={showCopies} onClick={() => setShowCopies(!showCopies)}>Cc / Bcc</button>
        {showCopies && <>{(["cc", "bcc"] as const).map((field) => <RecipientField key={field} id={field} label={field === "cc" ? "Cc" : "Bcc"} value={draft[field]} account={draft.account} disabled={busy} onChange={(value) => edit(field, value)} />)}</>}
        <label className="compose-field"><span>Subject</span><input aria-label="Subject" value={draft.subject} onChange={(e) => edit("subject", e.target.value)} disabled={busy} /></label>
        <div
          ref={bodyEditor}
          className="compose-body"
          role="textbox"
          aria-label="Message body"
          aria-multiline="true"
          aria-disabled={busy}
          contentEditable={!busy}
          suppressContentEditableWarning
          data-placeholder="Write your message…"
          dangerouslySetInnerHTML={{ __html: initialBodyHtml.current }}
          onInput={(event) => editBody(event.currentTarget)}
          onPaste={(event) => {
            const images = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith("image/"));
            if (images.length) {
              event.preventDefault();
              const selection = window.getSelection();
              const range = selection?.rangeCount && event.currentTarget.contains(selection.anchorNode) ? selection.getRangeAt(0).cloneRange() : null;
              images.forEach((image) => insertPastedImage(image, range?.cloneRange() ?? null));
              return;
            }
            event.preventDefault();
            document.execCommand("insertHTML", false, linkifyPlainText(event.clipboardData.getData("text/plain")));
          }}
          onClick={(event) => {
            const remove = (event.target as Element).closest<HTMLElement>("[data-compose-image-remove]");
            if (!remove) return;
            const wrapper = remove.closest<HTMLElement>("[data-compose-image]");
            const attachmentId = wrapper?.dataset.attachmentId;
            wrapper?.remove();
            editBody(event.currentTarget);
            event.currentTarget.focus();
            if (attachmentId) void run(async () => {
              await flush();
              const next = await mailClient.removeAttachment(latest.current.id, attachmentId);
              latest.current = next; setDraft(next);
            });
          }}
          onPointerDown={(event) => {
            const handle = (event.target as Element).closest<HTMLElement>("[data-compose-image-resize]");
            const wrapper = handle?.closest<HTMLElement>("[data-compose-image]");
            const image = wrapper?.querySelector("img");
            if (!handle || !wrapper || !image) return;
            event.preventDefault();
            const editor = event.currentTarget;
            const startX = event.clientX;
            const startWidth = wrapper.getBoundingClientRect().width || image.width || 320;
            const maximum = Math.max(80, editor.clientWidth);
            const move = (moveEvent: PointerEvent) => {
              const width = Math.round(Math.min(Math.max(startWidth + moveEvent.clientX - startX, 80), maximum));
              wrapper.style.width = `${width}px`;
              image.setAttribute("width", String(width));
              handle.setAttribute("aria-valuenow", String(width));
            };
            const finish = () => {
              window.removeEventListener("pointermove", move);
              window.removeEventListener("pointerup", finish);
              editBody(editor);
            };
            window.addEventListener("pointermove", move);
            window.addEventListener("pointerup", finish, { once: true });
          }}
          onKeyDown={(event) => {
            const resize = (event.target as Element).closest<HTMLElement>("[data-compose-image-resize]");
            if (resize && ["ArrowLeft", "ArrowRight"].includes(event.key)) {
              const wrapper = resize.closest<HTMLElement>("[data-compose-image]");
              const image = wrapper?.querySelector("img");
              if (!wrapper || !image) return;
              const direction = event.key === "ArrowRight" ? 1 : -1;
              const width = Math.min(Math.max((image.width || 320) + direction * (event.shiftKey ? 50 : 10), 80), event.currentTarget.clientWidth || 2000);
              wrapper.style.width = `${width}px`;
              image.setAttribute("width", String(width));
              resize.setAttribute("aria-valuenow", String(width));
              editBody(event.currentTarget);
              event.preventDefault();
              event.stopPropagation();
              return;
            }
            const shortcut = formattingShortcutFor(event.nativeEvent);
            if (!shortcut) return;
            if (applyFormattingShortcut(event.currentTarget, shortcut)) {
              event.preventDefault();
              event.stopPropagation();
              editBody(event.currentTarget);
            }
          }}
        />
        {draft.attachments.some((attachment) => !attachment.inline) && <ul className="attachment-list">{draft.attachments.filter((attachment) => !attachment.inline).map((a) => <li key={a.id}><span>{a.name} <small>{Math.ceil(a.size / 1024)} KB · {a.ready ? "Ready" : "Download required"}</small></span>{!a.ready && <button disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.fetchAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}>Download</button>}<button aria-label={`Remove ${a.name}`} disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.removeAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}><X size={14} /></button></li>)}</ul>}
        {error && <div className="compose-error" role="alert">{error} <button onClick={() => void run(async () => { await flush(); })}>Retry save</button></div>}
      </div>
      <footer><button className="send-button" onClick={() => send()} disabled={busy}><Send size={16} /> Send <kbd>⌘/Ctrl ↵</kbd></button><button onClick={attach} disabled={busy} aria-label="Attach files"><Paperclip size={17} /></button><span className="save-status" role="status">{status}</span><button disabled={busy} aria-label="Discard draft" onClick={() => void run(async () => { await flush(); await mailClient.discardDraft(draft.id); onClose(); })}><Trash2 size={16} /></button></footer>
      <p className="compose-note">Drafts are saved on this device. Send has a 10-second undo window.{!("__TAURI_INTERNALS__" in window) && " Browser preview: delivery and attachments are simulated."}</p>
  </div>;
});
