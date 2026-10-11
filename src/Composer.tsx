import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState } from "react";
import { Paperclip, Send, Sparkles, Trash, X } from "lucide-react";
import { mailClient } from "./data/client";
import type { Draft, OutboxItem } from "./correspondence";
import type { Account, ReplyAssistContext, Snippet } from "./domain";
import {
  isAiApiKeyConfigured,
  readAiFeatures,
  readAiProvider,
  readAiRequestConfig,
} from "./aiSettings";
import type { MessageAppearance } from "./SafeMessage";
import { RecipientField } from "./RecipientField";
import { Modal } from "./AppChrome";
import { ComposeBodyEditor, type ComposeBodyEditorHandle } from "./ComposeBodyEditor";
import { useDraftAutosave } from "./useDraftAutosave";
import { moveAddressesToBcc, replaceAddress } from "./composeChecks";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import type { DraftReviewActions } from "./draftReview";
import { plainTextToHtml } from "./richText";
import { ScheduleSendPicker } from "./ScheduleSendPicker";

export type ComposerHandle = {
  draftReview: DraftReviewActions;
  schedule(): void; flush(): Promise<Draft>; prepareExit(): Promise<void>; send(afterQueued?: () => void, archiveOnSend?: boolean): void; attach(): void; close(): void; discard(): void; draftReplyWithAI(): void;
  /** Inserts plain text where the caret last was in the body, or at the top when it never was. */
  insertText(text: string): void;
  /** Swaps one recipient address for another, in whichever field holds it. */
  replaceRecipient(from: string, to: string): void;
  /** Changes a new message's sending account. */
  switchAccount(email: string): void;
  /** Moves these addresses from To and Cc to Bcc. */
  moveRecipientsToBcc(emails: string[]): void;
  /** Focuses the body with the caret back where it was before the body lost focus. */
  focusBody(): void;
};

export const Composer = forwardRef<ComposerHandle, {
  draft: Draft;
  accounts: Account[];
  messageAppearance?: MessageAppearance;
  snippets: Snippet[];
  onCreateSnippet(name: string, body: string): Promise<Snippet>;
  onUpdateSnippet(id: string, name: string, body: string): Promise<Snippet>;
  onDeleteSnippet(id: string): Promise<void>;
  onClose(): void;
  onQueued(item: OutboxItem): void;
  availabilityText?: string | null;
  expandQuotedText?: boolean;
  replyAssistInstruction?: string | null;
  /** Called with the draft as edits are made; body text arrives at each autosave. */
  onDraftChange?(draft: Draft): void;
}>(function Composer({ draft: initial, accounts, messageAppearance, snippets, onCreateSnippet, onUpdateSnippet, onDeleteSnippet, onClose, onQueued, availabilityText = null, expandQuotedText = false, replyAssistInstruction = null, onDraftChange }, ref) {
  const bodyEditor = useRef<ComposeBodyEditorHandle>(null);
  const captureChanges = useCallback(() => bodyEditor.current?.captureChanges() ?? null, []);
  const [error, setError] = useState("");
  const { draft, latest, status, edit, markChanged, captureBody, flush, replaceDraft } = useDraftAutosave(initial, captureChanges, setError);
  const [scheduleOpen, setScheduleOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [showBlankCopies, setShowBlankCopies] = useState(false);
  const [replyAssistAvailable, setReplyAssistAvailable] = useState(false);
  const [replyAssistOpen, setReplyAssistOpen] = useState(false);
  const [replyAssistContext, setReplyAssistContext] = useState<ReplyAssistContext | null>(null);
  const [replyInstruction, setReplyInstruction] = useState("");
  const [replyAssistBusy, setReplyAssistBusy] = useState(false);
  const [replyAssistError, setReplyAssistError] = useState("");
  const replyInstructionInput = useRef<HTMLInputElement>(null);
  const automaticallyOpenedInstruction = useRef<string | null>(null);
  const [confirmAddToExisting, setConfirmAddToExisting] = useState(false);
  const panel = useRef<HTMLDivElement>(null);
  const pendingRecipientFocus = useRef<"cc" | "bcc" | null>(null);
  const mounted = useRef(true);
  const readInlineImage = useCallback((attachmentId: string) => mailClient.readInlineImage(initial.id, attachmentId), [initial.id]);

  async function run<T>(action: () => Promise<T>) {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError("");
    try { return await action(); } catch (e) { setError(String(e)); }
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
    if (scheduleOpen) return;
    void run(async () => {
      const saved = await flush();
      const item = archiveOnSend
        ? await mailClient.queueDraft(saved.id, saved.revision, true)
        : await mailClient.queueDraft(saved.id, saved.revision);
      onQueued(item);
      afterQueued?.();
    });
  }
  function attach() { void run(async () => { await flush(); const next = await mailClient.attachFiles(latest.current.id); replaceDraft(next); }); }
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
    void run(async () => { await flush(); const next = await mailClient.setDraftAccount(latest.current.id, email); replaceDraft(next); });
  }
  function insertText(text: string) {
    if (!busyRef.current) bodyEditor.current?.insertText(text);
  }
  function focusBody() { bodyEditor.current?.focusBody(); }
  function replaceRecipient(from: string, to: string) {
    for (const field of ["to", "cc", "bcc"] as const) {
      const next = replaceAddress(latest.current[field], from, to);
      if (next !== latest.current[field]) { edit(field, next); return; }
    }
  }
  function moveRecipientsToBcc(emails: string[]) {
    const changes = moveAddressesToBcc(latest.current, emails);
    for (const field of ["to", "cc", "bcc"] as const) {
      const value = changes[field];
      if (value !== undefined) edit(field, value);
    }
  }
  function draftReplyWithAI() {
    if (!replyAssistAvailable || replyAssistOpen) return;
    void openReplyAssist();
  }
  const openReplyAssist = useCallback(async () => {
    setReplyAssistOpen(true);
    setReplyAssistBusy(true);
    setReplyAssistError("");
    setReplyAssistContext(null);
    try {
      setReplyAssistContext(await mailClient.replyAssistContext(latest.current.id));
    } catch (reason) {
      setReplyAssistError(errorMessage(reason));
    } finally {
      if (mounted.current) setReplyAssistBusy(false);
    }
  }, [latest]);
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
      const { provider, model, endpoint, reasoning } = readAiRequestConfig("drafting a reply", "replyDraft");
      const result = await mailClient.generateReply(
        replyAssistContext,
        replyInstruction,
        provider,
        model,
        endpoint,
        reasoning,
      );
      if (!bodyEditor.current) return;
      bodyEditor.current.prependText(`${result.body.trim()}\n\n`);
      setReplyAssistOpen(false);
      setReplyInstruction("");
    } catch (reason) {
      setReplyAssistError(errorMessage(reason));
    } finally {
      if (mounted.current) setReplyAssistBusy(false);
    }
  }
  function attachInlineImage(file: File, data: string) {
    return run(async () => {
      await flush();
      const next = await mailClient.attachInlineImage(latest.current.id, file.name || "pasted-image", file.type, data);
      const attachment = next.attachments.find((candidate) => candidate.inline && !latest.current.attachments.some((existing) => existing.id === candidate.id));
      if (!attachment?.contentId) throw new Error("Could not prepare the pasted image");
      replaceDraft(next);
      return attachment;
    });
  }
  function removeInlineImage(attachmentId: string) {
    void run(async () => {
      await flush();
      replaceDraft(await mailClient.removeAttachment(latest.current.id, attachmentId));
    });
  }
  function readReviewDraft() {
    if (busyRef.current || !bodyEditor.current) throw new Error("Finish the current composer action first");
    captureBody();
    const authored = bodyEditor.current.reviewBody();
    const current = latest.current;
    return {
      id: current.id, subject: current.subject, body: authored.text, bodyHtml: authored.html,
      hasInlineImages: authored.hasInlineImages,
      hasInlineQuotes: authored.hasInlineQuotes,
      fingerprint: JSON.stringify([current.id, current.account, current.to, current.cc, current.bcc,
        current.subject, current.body, current.bodyHtml, current.attachments]),
    };
  }
  const draftReview: DraftReviewActions = {
    readDraft: readReviewDraft,
    replaceDraft: (expected, replacement) => {
      const current = readReviewDraft();
      if (current.id !== expected.id || current.fingerprint !== expected.fingerprint) {
        throw new Error("Your draft changed. Review it again before replacing any text.");
      }
      const html = "bodyHtml" in replacement ? replacement.bodyHtml
        : replacement.body === current.body ? current.bodyHtml : plainTextToHtml(replacement.body);
      if (html !== current.bodyHtml) {
        if (current.hasInlineImages || current.hasInlineQuotes) {
          throw new Error("Apply body suggestions manually to preserve inline images and quoted text.");
        }
        bodyEditor.current!.replaceAuthoredHtml(html);
      }
      edit("subject", replacement.subject);
      return readReviewDraft();
    },
  };
  useImperativeHandle(ref, () => ({ draftReview, flush, send, schedule: () => setScheduleOpen(true), attach, close, discard, draftReplyWithAI, insertText, replaceRecipient, moveRecipientsToBcc, switchAccount: changeAccount, focusBody, prepareExit: async () => {
    if (busyRef.current) throw new Error("Finish the current composer action before closing.");
    busyRef.current = true; setBusy(true);
    try { await flush(); }
    catch (e) { busyRef.current = false; setBusy(false); throw e; }
  } }));
  useEffect(() => {
    mounted.current = true;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panel.current?.querySelector<HTMLElement>(initial.mode === "new" || initial.mode === "forward" ? '[name="to"]' : '[contenteditable="true"]')?.focus();
    if (initial.mode === "forward" && initial.attachments.some((attachment) => !attachment.ready)) {
      void run(async () => {
        let next = await flush();
        for (const attachment of next.attachments.filter((candidate) => !candidate.ready)) {
          next = await mailClient.fetchAttachment(next.id, attachment.id);
          if (mounted.current) replaceDraft(next);
        }
      });
    }
    return () => { mounted.current = false; previous?.focus(); };
  }, [flush, initial.attachments, initial.mode, replaceDraft]);
  useEffect(() => {
    if (!["reply", "replyAll"].includes(initial.mode)) return;
    const enabled = readAiProvider() !== "none" && readAiFeatures().draftAssist;
    if (!enabled) return;
    void isAiApiKeyConfigured()
      .then((configured) => { if (mounted.current) setReplyAssistAvailable(configured); })
      .catch(() => { if (mounted.current) setReplyAssistAvailable(false); });
  }, [initial.mode]);
  useEffect(() => {
    if (!replyAssistInstruction || !replyAssistAvailable || automaticallyOpenedInstruction.current === replyAssistInstruction) return;
    automaticallyOpenedInstruction.current = replyAssistInstruction;
    setReplyInstruction(replyAssistInstruction);
    if (!replyAssistOpen) void openReplyAssist();
  }, [replyAssistAvailable, replyAssistInstruction, replyAssistOpen, openReplyAssist]);
  useEffect(() => {
    const field = pendingRecipientFocus.current;
    if (!field || !showBlankCopies) return;
    pendingRecipientFocus.current = null;
    panel.current?.querySelector<HTMLInputElement>(`[name="${field}"]`)?.focus();
  }, [showBlankCopies]);
  useEffect(() => { onDraftChange?.(draft); }, [draft, onDraftChange]);
  useEscapeDismiss(close, !scheduleOpen);
  return <div ref={panel} className={initial.mode === "new" ? "composer composer-inline composer-new" : "composer composer-inline"} role="dialog" data-shortcut-scope="compose" aria-label={initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward Message" : "Reply Message"}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing || replyAssistOpen || scheduleOpen) return;
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
      ) : <span>From {draft.account}</span>}</div><button className="btn-icon" aria-label="Save and Close Draft" onClick={close} disabled={busy}><X size={ICON_SIZE.lg} /></button></header>
      <div className="composer-content">
        <RecipientField id="to" label="To" value={draft.to} account={draft.account} disabled={busy} labelExpanded={showBlankCopies} onLabelClick={() => setShowBlankCopies((visible) => !visible)} onChange={(value) => edit("to", value)} />
        {(["cc", "bcc"] as const).map((field) => (
          (showBlankCopies || Boolean(draft[field].trim())) && (
            <RecipientField key={field} id={field} label={field === "cc" ? "Cc" : "Bcc"} value={draft[field]} account={draft.account} disabled={busy} onChange={(value) => edit(field, value)} />
          )
        ))}
        <label className="compose-field"><span>Subject</span><input aria-label="Subject" value={draft.subject} onChange={(e) => edit("subject", e.target.value)} disabled={busy} /></label>
        <ComposeBodyEditor
          ref={bodyEditor}
          initial={initial}
          attachments={draft.attachments}
          messageAppearance={messageAppearance}
          busy={busy}
          recipientTo={draft.to}
          availabilityText={availabilityText}
          expandQuotedText={expandQuotedText}
          snippets={snippets}
          onCreateSnippet={onCreateSnippet}
          onUpdateSnippet={onUpdateSnippet}
          onDeleteSnippet={onDeleteSnippet}
          onChange={markChanged}
          onError={setError}
          onAttachImage={attachInlineImage}
          onRemoveImage={removeInlineImage}
          readInlineImage={readInlineImage}
        />
        {replyAssistAvailable ? (
          <div className="reply-assist">
            <button type="button" className="btn reply-assist-trigger" onClick={() => void openReplyAssist()}>
              <Sparkles size={ICON_SIZE.md} /> Draft Reply With AI <kbd>⌘/Ctrl J</kbd>
            </button>
          </div>
        ) : null}
        {replyAssistInstruction && !replyAssistAvailable ? (
          <p className="reply-assist-task-context"><strong>Task-derived instruction:</strong> {replyAssistInstruction} Configure Reply Assist in AI settings to generate a suggestion.</p>
        ) : null}
        {draft.attachments.some((attachment) => !attachment.inline) && <ul className="attachment-list">{draft.attachments.filter((attachment) => !attachment.inline).map((a) => <li key={a.id}><span>{a.name} <small>{Math.ceil(a.size / 1024)} KB · {a.ready ? "Ready" : "Download required"}</small></span>{!a.ready && <button className="btn btn-sm" disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.fetchAttachment(draft.id, a.id); replaceDraft(next); })}>Download</button>}<button className="btn-icon btn-icon-sm" aria-label={`Remove ${a.name}`} disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.removeAttachment(draft.id, a.id); replaceDraft(next); })}><X size={ICON_SIZE.sm} /></button></li>)}</ul>}
        {error && <div className="notice compose-error" role="alert">{error} <button className="btn btn-sm" onClick={() => void run(async () => { await flush(); })}>Retry Save</button></div>}
      </div>
      <footer><button className="btn btn-primary send-button" onClick={() => send()} disabled={busy}><Send size={ICON_SIZE.md} /> Send <kbd>⌘/Ctrl ↵</kbd></button><button className="btn" onClick={() => setScheduleOpen(true)} disabled={busy}>Send later</button><button className="btn-icon" onClick={attach} disabled={busy} aria-label="Attach Files"><Paperclip size={ICON_SIZE.lg} /></button><span className="save-status" role="status">{status}</span><button className="btn-icon" disabled={busy} aria-label="Discard Draft" onClick={discard}><Trash size={ICON_SIZE.lg} /></button></footer>
      <p className="compose-note">Drafts are saved on this device. Send has a 10-second undo window.{!("__TAURI_INTERNALS__" in window) && " Browser preview: delivery and attachments are simulated."}</p>
      {scheduleOpen && <ScheduleSendPicker onClose={() => setScheduleOpen(false)} onSchedule={async (selection) => {
        if (busyRef.current) throw new Error("Finish the current composer action first");
        busyRef.current = true; setBusy(true);
        try { const saved = await flush(); const item = await mailClient.scheduleDraft(saved.id, saved.revision, selection); setScheduleOpen(false); onQueued(item); }
        finally { busyRef.current = false; if (mounted.current) setBusy(false); }
      }} />}
      {replyAssistOpen ? (
        <Modal title="Reply Assist" className="reply-assist-modal" backdropClassName="reply-assist-backdrop" initialFocusRef={replyInstructionInput} onClose={() => { setReplyAssistOpen(false); setConfirmAddToExisting(false); }}>
          <div className="reply-assist-panel">
            <label>
              <span>Optional Short Instruction</span>
              <input
                ref={replyInstructionInput}
                value={replyInstruction}
                placeholder="e.g. Accept and ask for available times"
                onChange={(event) => setReplyInstruction(event.target.value)}
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
                <button type="button" className="btn" onClick={() => setConfirmAddToExisting(false)}>Cancel</button>
                <button type="button" className="btn btn-primary" onClick={() => void generateReply(true)}>Add Anyway</button>
              </div>
            ) : null}
            {replyAssistError ? <div className="notice compose-error" role="alert">{replyAssistError}</div> : null}
            <div className="reply-assist-actions">
              <button
                type="button"
                className="btn btn-primary"
                disabled={replyAssistBusy || !replyAssistContext || confirmAddToExisting}
                onClick={() => void generateReply()}
              >
                {replyAssistBusy ? "Preparing…" : "Generate Draft"}
              </button>
              <span>The suggestion is never sent automatically.</span>
            </div>
          </div>
        </Modal>
      ) : null}
  </div>;
});

function replyBodyHasAuthoredContent(body: string): boolean {
  const quoteStart = body.search(/\n\nOn [\s\S]*? wrote:\n/);
  return (quoteStart >= 0 ? body.slice(0, quoteStart) : body).trim().length > 0;
}
