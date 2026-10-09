import { forwardRef, useCallback, useEffect, useImperativeHandle, useRef, useState, type ClipboardEvent as ReactClipboardEvent, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
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
import { RecipientField } from "./RecipientField";
import { Modal } from "./AppChrome";
import {
  applyListShortcut,
  applyFormattingShortcut,
  formattingShortcutFor,
  insertHtmlAtRange,
  linkifyPlainText,
  pastedLinkHref,
  draftTextToComposeHtml,
  plainTextToHtml,
  sanitizeComposeHtml,
  serializeComposeBody,
  splitReplyQuote,
} from "./richText";
import { SnippetPicker } from "./SnippetPicker";
import { moveAddressesToBcc, replaceAddress } from "./composeChecks";
import { normalizeAddressList } from "./emailAddress";
import { firstNameFromRecipient, renderSnippetBody } from "./snippets";
import { recordSnippetUsed } from "./settings";
import { useEscapeDismiss } from "./useEscapeDismiss";
import { errorMessage, logBackgroundFailure } from "./errors";
import { ICON_SIZE } from "./iconSizes";
import { closestFrom } from "./domTargets";

export type ComposerHandle = {
  flush(): Promise<Draft>; prepareExit(): Promise<void>; send(afterQueued?: () => void, archiveOnSend?: boolean): void; attach(): void; close(): void; discard(): void; draftReplyWithAI(): void;
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

// Typing pauses this long before a draft autosave.
const AUTOSAVE_DEBOUNCE_MS = 300;
// Upper bound on the unsaved window while editing never pauses.
const AUTOSAVE_INTERVAL_MS = 3_000;
const IMAGE_MIN_WIDTH = 80;
const IMAGE_MAX_WIDTH = 2000;
// Width assumed for an inline image whose size is not yet known.
const IMAGE_FALLBACK_WIDTH = 320;
const IMAGE_KEY_STEP = 10;
const IMAGE_KEY_LARGE_STEP = 50;

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
  /** Called with the draft as edits are made; body text arrives at each autosave. */
  onDraftChange?(draft: Draft): void;
}>(function Composer({ draft: initial, accounts, snippets, onCreateSnippet, onUpdateSnippet, onDeleteSnippet, onClose, onQueued, availabilityText = null, replyAssistInstruction = null, onDraftChange }, ref) {
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
  const replyInstructionInput = useRef<HTMLInputElement>(null);
  const automaticallyOpenedInstruction = useRef<string | null>(null);
  const insertedAvailabilityText = useRef<string | null>(null);
  const [confirmAddToExisting, setConfirmAddToExisting] = useState(false);
  const [snippetPickerOpen, setSnippetPickerOpen] = useState(false);
  const savedSnippetRange = useRef<Range | null>(null);
  // Where the caret was when the body last lost focus, so text inserted from
  // the context panel lands there rather than at the top.
  const lastBodyRange = useRef<Range | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const bodyEditor = useRef<HTMLDivElement>(null);
  // A reply's quoted history, edited apart from the body and collapsed by default.
  const quotedEditor = useRef<HTMLDivElement>(null);
  const [quoteExpanded, setQuoteExpanded] = useState(false);
  const bodyDirty = useRef(false);
  const pendingRecipientFocus = useRef<"cc" | "bcc" | null>(null);
  // Lazy initializer: `useRef(expr)` would evaluate `expr` on every render, so
  // each status change re-sanitized the whole quoted thread before the next paint.
  const [initialBody] = useState(() => {
    const html = sanitizeComposeHtml(initial.bodyHtml || draftTextToComposeHtml(initial.body));
    return splitReplyQuote(html) ?? { authoredHtml: html, quotedHtml: null };
  });
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mounted = useRef(true);

  const captureBody = useCallback(() => {
    const editor = bodyEditor.current;
    if (!bodyDirty.current || !editor) return;
    const { html, text } = serializeComposeBody(editor, quotedEditor.current);
    latest.current = {
      ...latest.current,
      body: text,
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
    // Saved recipients are always well-formed header text, which also repairs
    // a draft holding an unquoted comma in a name.
    const snapshot = { ...latest.current, to: normalizeAddressList(latest.current.to), cc: normalizeAddressList(latest.current.cc), bcc: normalizeAddressList(latest.current.bcc) };
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
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_DEBOUNCE_MS);
  }
  const editBody = useCallback(() => {
    // Keep the browser-owned contenteditable DOM off React's render path.
    // Cloning and sanitizing a long reply on every input made typing cost grow
    // with the entire quoted thread; capture it only at an autosave boundary.
    bodyDirty.current = true;
    generation.current++;
    setStatus("Unsaved changes");
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_DEBOUNCE_MS);
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
  function insertText(text: string) {
    const editor = bodyEditor.current;
    if (!editor || busyRef.current) return;
    const saved = lastBodyRange.current;
    const range = saved && editor.contains(saved.startContainer) ? saved : document.createRange();
    if (range !== saved) { range.selectNodeContents(editor); range.collapse(true); }
    editor.focus();
    insertHtmlAtRange(editor, range, sanitizeComposeHtml(plainTextToHtml(`${text}\n`)));
    lastBodyRange.current = null;
    editBody();
  }
  function focusBody() {
    const editor = bodyEditor.current;
    if (!editor) return;
    editor.focus();
    const saved = lastBodyRange.current;
    if (!saved || !editor.contains(saved.startContainer)) return;
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(saved);
  }
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
  async function openReplyAssist() {
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
    resize.setAttribute("aria-valuemin", String(IMAGE_MIN_WIDTH));
    resize.setAttribute("aria-valuemax", String(IMAGE_MAX_WIDTH));
    resize.setAttribute("aria-valuenow", String(image.width || IMAGE_FALLBACK_WIDTH));
    resize.tabIndex = 0;
    resize.title = "Drag to resize";
    wrapper.append(resize);
  }
  function insertPastedImage(editor: HTMLElement, file: File, range: Range | null) {
    const reader = new FileReader();
    reader.onerror = () => setError(`Could not paste ${file.name || "image"}.`);
    reader.onload = () => {
      if (!mounted.current || typeof reader.result !== "string" || !editor.isConnected) return;
      const preview = reader.result;
      const data = preview.slice(preview.indexOf(",") + 1);
      void run(async () => {
        await flush();
        const next = await mailClient.attachInlineImage(latest.current.id, file.name || "pasted-image", file.type, data);
        const attachment = next.attachments.find((candidate) => candidate.inline && !latest.current.attachments.some((existing) => existing.id === candidate.id));
        if (!attachment?.contentId || !editor.isConnected) throw new Error("Could not prepare the pasted image");
        latest.current = next; setDraft(next);

        const image = document.createElement("img");
        image.src = preview;
        image.dataset.composeSource = `cid:${attachment.contentId}`;
        image.alt = file.name || "Pasted image";
        decorateImage(image, attachment.id);
        const wrapper = image.closest<HTMLElement>("[data-compose-image]")!;
        const insertion = range && editor.contains(range.commonAncestorContainer) ? range : document.createRange();
        if (!range || !editor.contains(range.commonAncestorContainer)) insertion.selectNodeContents(editor);
        insertion.collapse(false);
        insertion.deleteContents();
        insertion.insertNode(wrapper);
        const spacer = document.createTextNode("\u00a0");
        wrapper.after(spacer);
        const caret = document.createRange();
        caret.setStartAfter(spacer); caret.collapse(true);
        window.getSelection()?.removeAllRanges(); window.getSelection()?.addRange(caret);
        image.onload = () => {
          const available = editor.clientWidth;
          const width = Math.max(IMAGE_MIN_WIDTH, Math.min(image.naturalWidth, available));
          if (width) { image.setAttribute("width", String(Math.round(width))); wrapper.style.width = `${Math.round(width)}px`; }
          editBody();
        };
        editBody();
      });
    };
    reader.readAsDataURL(file);
  }
  useImperativeHandle(ref, () => ({ flush, send, attach, close, discard, draftReplyWithAI, insertText, replaceRecipient, moveRecipientsToBcc, switchAccount: changeAccount, focusBody, prepareExit: async () => {
    if (busyRef.current) throw new Error("Finish the current composer action before closing.");
    busyRef.current = true; setBusy(true);
    try { await flush(); }
    catch (e) { busyRef.current = false; setBusy(false); throw e; }
  } }));
  useEffect(() => {
    mounted.current = true;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    [bodyEditor.current, quotedEditor.current].flatMap((editor) => Array.from(editor?.querySelectorAll<HTMLImageElement>("img") ?? [])).forEach((image) => {
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
    const autosave = window.setInterval(() => { void flush().catch(logBackgroundFailure("Draft autosave")); }, AUTOSAVE_INTERVAL_MS);
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
    if (!replyAssistInstruction || !replyAssistAvailable || automaticallyOpenedInstruction.current === replyAssistInstruction) return;
    automaticallyOpenedInstruction.current = replyAssistInstruction;
    setReplyInstruction(replyAssistInstruction);
    if (!replyAssistOpen) void openReplyAssist();
  }, [replyAssistAvailable, replyAssistInstruction, replyAssistOpen]);
  useEffect(() => {
    const field = pendingRecipientFocus.current;
    if (!field || !showBlankCopies) return;
    pendingRecipientFocus.current = null;
    panel.current?.querySelector<HTMLInputElement>(`[name="${field}"]`)?.focus();
  }, [showBlankCopies]);
  useEffect(() => { onDraftChange?.(draft); }, [draft, onDraftChange]);
  useEscapeDismiss(close);
  // Editing behavior shared by the body and a reply's quoted history.
  const editingProps = {
    contentEditable: !busy,
    suppressContentEditableWarning: true,
    onInput: editBody,
    onPaste: (event: ReactClipboardEvent<HTMLDivElement>) => {
      const images = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith("image/"));
      if (images.length) {
        event.preventDefault();
        const selection = window.getSelection();
        const editor = event.currentTarget;
        const range = selection?.rangeCount && editor.contains(selection.anchorNode) ? selection.getRangeAt(0).cloneRange() : null;
        images.forEach((image) => insertPastedImage(editor, image, range?.cloneRange() ?? null));
        return;
      }
      event.preventDefault();
      const text = event.clipboardData.getData("text/plain");
      const selection = window.getSelection();
      const href = pastedLinkHref(text);
      if (href && selection && !selection.isCollapsed && selection.toString().trim() && event.currentTarget.contains(selection.anchorNode)) {
        document.execCommand("createLink", false, href);
        return;
      }
      document.execCommand("insertHTML", false, linkifyPlainText(text));
    },
    onClick: (event: ReactMouseEvent<HTMLDivElement>) => {
      const remove = closestFrom<HTMLElement>(event.target, "[data-compose-image-remove]");
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
    },
    onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => {
      const handle = closestFrom<HTMLElement>(event.target, "[data-compose-image-resize]");
      const wrapper = handle?.closest<HTMLElement>("[data-compose-image]");
      const image = wrapper?.querySelector("img");
      if (!handle || !wrapper || !image) return;
      event.preventDefault();
      const editor = event.currentTarget;
      const startX = event.clientX;
      const startWidth = wrapper.getBoundingClientRect().width || image.width || IMAGE_FALLBACK_WIDTH;
      const maximum = Math.max(IMAGE_MIN_WIDTH, editor.clientWidth);
      const move = (moveEvent: PointerEvent) => {
        const width = Math.round(Math.min(Math.max(startWidth + moveEvent.clientX - startX, IMAGE_MIN_WIDTH), maximum));
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
    },
    onKeyDown: (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ";") {
        event.preventDefault();
        const selection = window.getSelection();
        savedSnippetRange.current = selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
        setSnippetPickerOpen(true);
        return;
      }
      const resize = closestFrom<HTMLElement>(event.target, "[data-compose-image-resize]");
      if (resize && ["ArrowLeft", "ArrowRight"].includes(event.key)) {
        const wrapper = resize.closest<HTMLElement>("[data-compose-image]");
        const image = wrapper?.querySelector("img");
        if (!wrapper || !image) return;
        const direction = event.key === "ArrowRight" ? 1 : -1;
        const width = Math.min(Math.max((image.width || IMAGE_FALLBACK_WIDTH) + direction * (event.shiftKey ? IMAGE_KEY_LARGE_STEP : IMAGE_KEY_STEP), IMAGE_MIN_WIDTH), event.currentTarget.clientWidth || IMAGE_MAX_WIDTH);
        wrapper.style.width = `${width}px`;
        image.setAttribute("width", String(width));
        resize.setAttribute("aria-valuenow", String(width));
        editBody();
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (!event.nativeEvent.isComposing && event.key === " " && applyListShortcut(event.currentTarget)) {
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
    },
  };
  return <div ref={panel} className={initial.mode === "new" ? "composer composer-inline composer-new" : "composer composer-inline"} role="dialog" data-shortcut-scope="compose" aria-label={initial.mode === "new" ? "New Message" : initial.mode === "forward" ? "Forward Message" : "Reply Message"}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing || replyAssistOpen) return;
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
        <div
          ref={bodyEditor}
          className="compose-body"
          role="textbox"
          aria-label="Message Body"
          aria-multiline="true"
          aria-disabled={busy}
          data-placeholder="Write your message…"
          dangerouslySetInnerHTML={{ __html: initialBody.authoredHtml }}
          {...editingProps}
          onBlur={() => {
            const selection = window.getSelection();
            const range = selection?.rangeCount ? selection.getRangeAt(0) : null;
            lastBodyRange.current = range && bodyEditor.current?.contains(range.startContainer) ? range.cloneRange() : null;
          }}
        />
        {initialBody.quotedHtml !== null ? (
          <>
            <button
              type="button"
              className="btn btn-sm compose-quote-toggle"
              aria-label={quoteExpanded ? "Hide Quoted Text" : "Show Quoted Text"}
              aria-expanded={quoteExpanded}
              aria-controls={`${initial.id}-quoted`}
              title={quoteExpanded ? "Hide quoted text" : "Show quoted text"}
              onClick={() => setQuoteExpanded((expanded) => !expanded)}
            >···</button>
            {/* Collapsed with `hidden`, so assistive and dictation software
                reading the page skip the history until it is shown. */}
            <div
              ref={quotedEditor}
              id={`${initial.id}-quoted`}
              className="compose-body compose-quoted"
              role="textbox"
              aria-label="Quoted Text"
              aria-multiline="true"
              aria-disabled={busy}
              hidden={!quoteExpanded}
              dangerouslySetInnerHTML={{ __html: initialBody.quotedHtml }}
              {...editingProps}
            />
          </>
        ) : null}
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
        {draft.attachments.some((attachment) => !attachment.inline) && <ul className="attachment-list">{draft.attachments.filter((attachment) => !attachment.inline).map((a) => <li key={a.id}><span>{a.name} <small>{Math.ceil(a.size / 1024)} KB · {a.ready ? "Ready" : "Download required"}</small></span>{!a.ready && <button className="btn btn-sm" disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.fetchAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}>Download</button>}<button className="btn-icon btn-icon-sm" aria-label={`Remove ${a.name}`} disabled={busy} onClick={() => void run(async () => { await flush(); const next = await mailClient.removeAttachment(draft.id, a.id); latest.current = next; setDraft(next); })}><X size={ICON_SIZE.sm} /></button></li>)}</ul>}
        {error && <div className="notice compose-error" role="alert">{error} <button className="btn btn-sm" onClick={() => void run(async () => { await flush(); })}>Retry Save</button></div>}
      </div>
      <footer><button className="btn btn-primary send-button" onClick={() => send()} disabled={busy}><Send size={ICON_SIZE.md} /> Send <kbd>⌘/Ctrl ↵</kbd></button><button className="btn-icon" onClick={attach} disabled={busy} aria-label="Attach Files"><Paperclip size={ICON_SIZE.lg} /></button><span className="save-status" role="status">{status}</span><button className="btn-icon" disabled={busy} aria-label="Discard Draft" onClick={discard}><Trash size={ICON_SIZE.lg} /></button></footer>
      <p className="compose-note">Drafts are saved on this device. Send has a 10-second undo window.{!("__TAURI_INTERNALS__" in window) && " Browser preview: delivery and attachments are simulated."}</p>
      {snippetPickerOpen ? (
        <SnippetPicker
          snippets={snippets}
          onClose={() => setSnippetPickerOpen(false)}
          onInsert={(snippet) => {
            recordSnippetUsed(snippet.id);
            setSnippetPickerOpen(false);
            const saved = savedSnippetRange.current;
            const editor = [bodyEditor.current, quotedEditor.current].find((candidate) => candidate && saved && candidate.contains(saved.startContainer)) ?? bodyEditor.current;
            if (!editor) return;
            editor.focus();
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
