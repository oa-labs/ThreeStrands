import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Paperclip, Send, Sparkles, Trash2, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { Draft, OutboxItem } from "./correspondence";
import type { Account, ReplyAssistContext, Snippet } from "./domain";
import {
  isAiApiKeyConfigured,
  readAiFeatures,
  readAiProvider,
  readAiRequestConfig,
} from "./aiSettings";
import { RecipientField } from "./RecipientField";
import {
  applyAsteriskListShortcut,
  applyFormattingShortcut,
  formattingShortcutFor,
  insertHtmlAtRange,
  linkifyPlainText,
  plainTextToHtml,
  sanitizeComposeHtml,
  serializeComposeHtml,
} from "./richText";
import { SnippetPicker } from "./SnippetPicker";
import { firstNameFromRecipient, renderSnippetBody } from "./snippets";
import { recordSnippetUsed } from "./settings";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage, logBackgroundFailure } from "./errors";

export type ComposerHandle = { flush(): Promise<Draft>; prepareExit(): Promise<void>; send(afterQueued?: () => void, archiveOnSend?: boolean): void; attach(): void; close(): void; discard(): void; draftReplyWithAI(): void };

export const Composer = forwardRef<ComposerHandle, {
  draft: Draft;
  accounts: Account[];
  snippets: Snippet[];
  onCreateSnippet(name: string, body: string): Promise<Snippet>;
  onUpdateSnippet(id: string, name: string, body: string): Promise<Snippet>;
  onDeleteSnippet(id: string): Promise<void>;
  onClose(): void;
  onQueued(item: OutboxItem): void;
  availabilityText?: string | null;
  replyAssistInstruction?: string | null;
}>(function Composer({ draft: initial, accounts, snippets, onCreateSnippet, onUpdateSnippet, onDeleteSnippet, onClose, onQueued, availabilityText = null, replyAssistInstruction = null }, ref) {
  const [draft, setDraft] = useState(initial);
  const latest = useRef(initial);
  const generation = useRef(0);
  const savedGeneration = useRef(0);
  const pending = useRef<Promise<Draft> | null>(null);
  const [status, setStatus] = useState("Saved on this device");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [showBlankCopies, setShowBlankCopies] = useState(false);
  const [replyAssistAvailable, setReplyAssistAvailable] = useState(false);
  const [replyAssistOpen, setReplyAssistOpen] = useState(false);
  const [replyAssistContext, setReplyAssistContext] = useState<ReplyAssistContext | null>(null);
  const [replyInstruction, setReplyInstruction] = useState("");
  const [replyAssistBusy, setReplyAssistBusy] = useState(false);
  const [replyAssistError, setReplyAssistError] = useState("");
  const insertedAvailabilityText = useRef<string | null>(null);
  const [confirmAddToExisting, setConfirmAddToExisting] = useState(false);
  const [snippetPickerOpen, setSnippetPickerOpen] = useState(false);
  const savedSnippetRange = useRef<Range | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const bodyEditor = useRef<HTMLDivElement>(null);
  const bodyDirty = useRef(false);
  const pendingRecipientFocus = useRef<"cc" | "bcc" | null>(null);
  const initialBodyHtml = useRef(sanitizeComposeHtml(initial.bodyHtml || plainTextToHtml(initial.body)));
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);

  const captureBody = useCallback(() => {
    const editor = bodyEditor.current;
    if (!bodyDirty.current || !editor) return;
    const html = serializeComposeHtml(editor);
    const textOnly = editor.cloneNode(true) as HTMLElement;
    textOnly.querySelectorAll("[data-compose-image-remove], [data-compose-image-resize]").forEach((control) => control.remove());
    latest.current = {
      ...latest.current,
      body: textOnly.innerText ?? textOnly.textContent ?? "",
      bodyHtml: html,
    };
    bodyDirty.current = false;
  }, []);

  const flush = useCallback(function flush(): Promise<Draft> {
    if (timer.current) clearTimeout(timer.current);
    captureBody();
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
  }, [captureBody]);
  function edit(field: "to" | "cc" | "bcc" | "subject" | "body", value: string) {
    latest.current = { ...latest.current, [field]: value }; generation.current++;
    setDraft(latest.current); setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, 300);
  }
  const editBody = useCallback(() => {
    // Keep the browser-owned contenteditable DOM off React's render path.
    // Cloning and sanitizing a long reply on every input made typing cost grow
    // with the entire quoted thread; capture it only at an autosave boundary.
    bodyDirty.current = true;
    generation.current++;
    setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, 300);
  }, [flush]);
  async function run(action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError("");
    try { await action(); } catch (e) { setError(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  function close() {
    void run(async () => {
      const saved = await flush();
      const isEmpty = !saved.to.trim()
        && !saved.cc.trim()
        && !saved.bcc.trim()
        && !saved.subject.trim()
        && !saved.body.trim()
        && !saved.bodyHtml?.trim()
        && saved.attachments.length === 0;
      if (isEmpty) await mailClient.discardDraft(saved.id);
      onClose();
    });
  }
  function discard() { void run(async () => { await flush(); await mailClient.discardDraft(draft.id); onClose(); }); }
  function send(afterQueued?: () => void, archiveOnSend?: boolean) {
    void run(async () => {
      const saved = await flush();
      const item = archiveOnSend
        ? await mailClient.queueDraft(saved.id, saved.revision, true)
        : await mailClient.queueDraft(saved.id, saved.revision);
      onQueued(item);
      afterQueued?.();
    });
  }
  function attach() { void run(async () => { await flush(); const next = await mailClient.attachFiles(latest.current.id); latest.current = next; setDraft(next); }); }
  function focusRecipient(field: "to" | "cc" | "bcc") {
    const input = panel.current?.querySelector<HTMLInputElement>(`[name="${field}"]`);
    if (input) {
      input.focus();
      return;
    }
    pendingRecipientFocus.current = field === "to" ? null : field;
    setShowBlankCopies(true);
  }
  function changeAccount(email: string) {
    if (email === latest.current.account) return;
    void run(async () => { await flush(); const next = await mailClient.setDraftAccount(latest.current.id, email); latest.current = next; setDraft(next); });
  }
  function draftReplyWithAI() {
    if (!replyAssistAvailable || replyAssistOpen) return;
    void openReplyAssist();
  }
  async function openReplyAssist() {
    setReplyAssistOpen(true);
    setReplyAssistBusy(true);
    setReplyAssistError("");
    try {
      setReplyAssistContext(await mailClient.replyAssistContext(latest.current.id));
    } catch (reason) {
      setReplyAssistError(errorMessage(reason));
    } finally {
      if (mounted.current) setReplyAssistBusy(false);
    }
  }
  async function generateReply(confirmed = false) {
    if (!replyAssistContext || !bodyEditor.current) return;
    captureBody();
    if (replyBodyHasAuthoredContent(latest.current.body) && !confirmed) {
      setConfirmAddToExisting(true);
      return;
    }
    setConfirmAddToExisting(false);
    setReplyAssistBusy(true);
    setReplyAssistError("");
    try {
      const { provider, model, endpoint } = readAiRequestConfig("drafting a reply");
      const result = await mailClient.generateReply(
        replyAssistContext,
        replyInstruction,
        provider,
        model,
        endpoint,
      );
      if (!bodyEditor.current) return;
      // The provider's output is always handled as text. `plainTextToHtml`
      // escapes markup before insertion, and the composer sanitizer remains
      // the final defense before the draft is persisted.
      const generatedHtml = sanitizeComposeHtml(plainTextToHtml(`${result.body.trim()}\n\n`));
      bodyEditor.current.insertAdjacentHTML("afterbegin", generatedHtml);
      editBody();
      setReplyAssistOpen(false);
      setReplyInstruction("");
    } catch (reason) {
      setReplyAssistError(errorMessage(reason));
    } finally {
      if (mounted.current) setReplyAssistBusy(false);
    }
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
    remove.setAttribute("aria-label", "Remove Pasted Image");
    remove.title = "Remove image";
    remove.textContent = "×";
    wrapper.append(remove);

    const resize = document.createElement("span");
    resize.className = "compose-image-resize";
    resize.dataset.composeImageResize = "true";
    resize.setAttribute("role", "slider");
    resize.setAttribute("aria-label", "Resize Pasted Image");
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
          editBody();
        };
        editBody();
      });
    };
    reader.readAsDataURL(file);
  }
  useImperativeHandle(ref, () => ({ flush, send, attach, close, discard, draftReplyWithAI, prepareExit: async () => {
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
    // Safety net alongside the keystroke-debounced save in `edit`/`editBody`:
    // continuous typing/dictation keeps resetting that debounce, so bound
    // the worst-case unsaved window regardless of how long editing continues.
    // `flush()` already no-ops if nothing changed since the last save.
    const autosave = window.setInterval(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, 3_000);
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
    return () => { mounted.current = false; if (timer.current) clearTimeout(timer.current); window.clearInterval(autosave); window.removeEventListener("beforeunload", beforeUnload); previous?.focus(); };
  }, [flush, initial.attachments, initial.id, initial.mode]);
  useEffect(() => {
    if (!availabilityText || !bodyEditor.current || insertedAvailabilityText.current === availabilityText) return;
    insertedAvailabilityText.current = availabilityText;
    bodyEditor.current.insertAdjacentHTML(
      "afterbegin",
      sanitizeComposeHtml(plainTextToHtml(`${availabilityText}\n\n`)),
    );
    editBody();
  }, [availabilityText, editBody]);
  useEffect(() => {
    if (!["reply", "replyAll"].includes(initial.mode)) return;
    const enabled = readAiProvider() !== "none" && readAiFeatures().draftAssist;
    if (!enabled) return;
    void isAiApiKeyConfigured()
      .then((configured) => { if (mounted.current) setReplyAssistAvailable(configured); })
      .catch(() => { if (mounted.current) setReplyAssistAvailable(false); });
  }, [initial.mode]);
  useEffect(() => {
    if (!replyAssistInstruction || !replyAssistAvailable) return;
    setReplyInstruction(replyAssistInstruction);
    if (!replyAssistOpen) void openReplyAssist();
  }, [replyAssistAvailable, replyAssistInstruction, replyAssistOpen]);
  useEffect(() => {
    const field = pendingRecipientFocus.current;
    if (!field || !showBlankCopies) return;
    pendingRecipientFocus.current = null;
    panel.current?.querySelector<HTMLInputElement>(`[name="${field}"]`)?.focus();
  }, [showBlankCopies]);
  useEscapeDismiss(close);
  return <div ref={panel} className="composer composer-inline" role="dialog" data-shortcut-scope="compose" aria-label={initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward Message" : "Reply Message"}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if ((event.metaKey || event.ctrlKey) && event.shiftKey && ["o", "c", "b"].includes(event.key.toLowerCase())) {
          const field = event.key.toLowerCase() === "o" ? "to" : event.key.toLowerCase() === "c" ? "cc" : "bcc";
          event.preventDefault();
          focusRecipient(field);
          return;
        }
        if (event.key === "Tab") {
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [contenteditable="true"], [tabindex="0"]') ?? []);
          const first = controls[0], last = controls.at(-1);
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <header className="composer-header"><div><h2>{initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward" : initial.mode === "replyAll" ? "Reply All" : "Reply"}</h2>{initial.mode === "new" && accounts.length > 1 ? (
        <label className="compose-from"><span>From</span><select aria-label="Send From" value={draft.account} disabled={busy} onChange={(e) => changeAccount(e.target.value)}>
          {accounts.map((a) => <option key={a.email} value={a.email}>{a.email}</option>)}
        </select></label>
      ) : <span>From {draft.account}</span>}</div><button className="icon-button" aria-label="Save and Close Draft" onClick={close} disabled={busy}><X size={19} /></button></header>
      <div className="composer-content">
        <RecipientField id="to" label="To" value={draft.to} account={draft.account} disabled={busy} labelExpanded={showBlankCopies} onLabelClick={() => setShowBlankCopies((visible) => !visible)} onChange={(value) => edit("to", value)} />
        {(["cc", "bcc"] as const).map((field) => (
          (showBlankCopies || Boolean(draft[field].trim())) && (
            <RecipientField key={field} id={field} label={field === "cc" ? "Cc" : "Bcc"} value={draft[field]} account={draft.account} disabled={busy} onChange={(value) => edit(field, value)} />
          )
        ))}
        <label className="compose-field"><span>Subject</span><input aria-label="Subject" value={draft.subject} onChange={(e) => edit("subject", e.target.value)} disabled={busy} /></label>
        <div
          ref={bodyEditor}
          className="compose-body"
          role="textbox"
          aria-label="Message Body"
          aria-multiline="true"
          aria-disabled={busy}
          contentEditable={!busy}
          suppressContentEditableWarning
          data-placeholder="Write your message…"
          dangerouslySetInnerHTML={{ __html: initialBodyHtml.current }}
          onInput={editBody}
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
            editBody();
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
              editBody();
            };
            window.addEventListener("pointermove", move);
            window.addEventListener("pointerup", finish, { once: true });
          }}
          onKeyDown={(event) => {
            if ((event.metaKey || event.ctrlKey) && event.key === ";") {
              event.preventDefault();
              const selection = window.getSelection();
              savedSnippetRange.current = selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
              setSnippetPickerOpen(true);
              return;
            }
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
              editBody();
              event.preventDefault();
              event.stopPropagation();
              return;
            }
            if (!event.nativeEvent.isComposing && event.key === " " && applyAsteriskListShortcut(event.currentTarget)) {
              event.preventDefault();
              event.stopPropagation();
              editBody();
              return;
            }
            const shortcut = formattingShortcutFor(event.nativeEvent);
            if (!shortcut) return;
            if (applyFormattingShortcut(event.currentTarget, shortcut)) {
              event.preventDefault();
              event.stopPropagation();
              editBody();
            }
          }}
        />
        {replyAssistAvailable ? (
          <div className="reply-assist">
            {!replyAssistOpen ? (
              <button type="button" className="reply-assist-trigger" onClick={() => void openReplyAssist()}>
                <Sparkles size={14} /> Draft Reply With AI <kbd>⌘/Ctrl J</kbd>
              </button>
            ) : (
              <section className="reply-assist-panel" aria-label="Reply Assist">
                <div className="reply-assist-heading">
                  <strong><Sparkles size={14} /> Reply Assist</strong>
                  <button type="button" aria-label="Close Reply Assist" onClick={() => setReplyAssistOpen(false)}>
                    <X size={14} />
                  </button>
                </div>
                <label>
                  <span>Optional Short Instruction</span>
                  <input
                    value={replyInstruction}
                    placeholder="e.g. Accept and ask for available times"
                    onChange={(event) => setReplyInstruction(event.target.value)}
                    disabled={replyAssistBusy}
                  />
                </label>
                {replyAssistInstruction ? <p className="reply-assist-task-context"><strong>Task-derived instruction:</strong> {replyAssistInstruction}</p> : null}
                {replyAssistContext ? (
                  <details open className="reply-assist-context">
                    <summary>Exact email content sent to {readAiProvider()}</summary>
                    <div className="reply-assist-context-body">
                      <p><strong>Subject:</strong> {replyAssistContext.subject}</p>
                      {replyAssistContext.messages.map((message, index) => (
                        <article key={`${message.sentAt}-${index}`}>
                          <p><strong>From:</strong> {message.sender}</p>
                          <p><strong>Date:</strong> {message.sentAt}</p>
                          <pre>{message.bodyText}</pre>
                        </article>
                      ))}
                    </div>
                  </details>
                ) : null}
                {confirmAddToExisting ? (
                  <div className="reply-assist-confirm" role="alert">
                    <span>Your reply already contains text. The suggestion will be added above it without replacing anything.</span>
                    <button type="button" onClick={() => void generateReply(true)}>Add Anyway</button>
                    <button type="button" onClick={() => setConfirmAddToExisting(false)}>Cancel</button>
                  </div>
                ) : null}
                {replyAssistError ? <div className="notice compose-error" role="alert">{replyAssistError}</div> : null}
                <div className="reply-assist-actions">
                  <button
                    type="button"
                    disabled={replyAssistBusy || !replyAssistContext || confirmAddToExisting}
                    onClick={() => void generateReply()}
                  >
                    {replyAssistBusy ? "Preparing…" : "Generate Draft"}
                  </button>
                  <span>The suggestion is never sent automatically.</span>
                </div>
              </section>
            )}
          </div>
        ) : null}
        {replyAssistInstruction && !replyAssistOpen ? (
          <p className="reply-assist-task-context"><strong>Task-derived instruction:</strong> {replyAssistInstruction} Configure Reply Assist in AI settings to generate a suggestion.</p>
        ) : null}
        {draft.attachments.some((attachment) => !attachment.inline) && <ul className="attachment-list">{draft.attachments.filter((attachment) => !attachment.inline).map((a) => <li key={a.id}><span>{a.name} <small>{Math.ceil(a.size / 1024)} KB · {a.ready ? "Ready" : "Download required"}</small></span>{!a.ready && <button disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.fetchAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}>Download</button>}<button aria-label={`Remove ${a.name}`} disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.removeAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}><X size={14} /></button></li>)}</ul>}
        {error && <div className="notice compose-error" role="alert">{error} <button onClick={() => void run(async () => { await flush(); })}>Retry Save</button></div>}
      </div>
      <footer><button className="send-button" onClick={() => send()} disabled={busy}><Send size={16} /> Send <kbd>⌘/Ctrl ↵</kbd></button><button onClick={attach} disabled={busy} aria-label="Attach Files"><Paperclip size={17} /></button><span className="save-status" role="status">{status}</span><button disabled={busy} aria-label="Discard Draft" onClick={discard}><Trash2 size={16} /></button></footer>
      <p className="compose-note">Drafts are saved on this device. Send has a 10-second undo window.{!("__TAURI_INTERNALS__" in window) && " Browser preview: delivery and attachments are simulated."}</p>
      {snippetPickerOpen ? (
        <SnippetPicker
          snippets={snippets}
          onClose={() => setSnippetPickerOpen(false)}
          onInsert={(snippet) => {
            recordSnippetUsed(snippet.id);
            setSnippetPickerOpen(false);
            const editor = bodyEditor.current;
            if (!editor) return;
            editor.focus();
            const saved = savedSnippetRange.current;
            let range: Range;
            if (saved && editor.contains(saved.startContainer)) {
              range = saved;
            } else {
              range = document.createRange();
              range.selectNodeContents(editor);
              range.collapse(false);
            }
            const rendered = renderSnippetBody(snippet.body, { firstName: firstNameFromRecipient(draft.to) });
            insertHtmlAtRange(editor, range, rendered);
            editBody();
          }}
          onCreate={onCreateSnippet}
          onUpdate={onUpdateSnippet}
          onDelete={onDeleteSnippet}
        />
      ) : null}
  </div>;
});

function replyBodyHasAuthoredContent(body: string): boolean {
  const quoteStart = body.search(/\n\nOn [\s\S]*? wrote:\n/);
  return (quoteStart >= 0 ? body.slice(0, quoteStart) : body).trim().length > 0;
}
