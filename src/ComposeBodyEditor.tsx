import { loadSignatureEditorImages } from "./signatureEditorImages";
import { readSignatureHtml, readSignatureEditor, signatureEditorHtml, type DraftSignature } from "./signatures";
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type ReactNode, type ClipboardEvent as ReactClipboardEvent, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import type { Attachment, Draft } from "./correspondence";
import type { Snippet } from "./domain";
import {
  applyCodeBlockEnter, applyListShortcut, applyFormattingShortcut, formattingShortcutFor, forwardedComposeHtml,
  insertHtmlAtRange, linkifyPlainText, pastedLinkHref, draftTextToComposeHtml,
  plainTextToHtml, sanitizeComposeHtml, serializeComposeBody, splitReplyQuote,
  type FormattingShortcut,
} from "./richText";
import { FormattingPromptDialog } from "./FormattingPromptDialog";
import { InlineConfirm } from "./InlineConfirm";
import { SafeMessage, type MessageAppearance } from "./SafeMessage";
import { mailClient } from "./data/client";
import { normalizeContentId } from "./inlineAttachments";
import { SnippetPicker } from "./SnippetPicker";
import { firstNameFromRecipient, renderSnippetBody } from "./snippets";
import { recordSnippetUsed } from "./settings";
import { errorMessage } from "./errors";
import { closestFrom } from "./domTargets";

const IMAGE_MIN_WIDTH = 80;
const IMAGE_MAX_WIDTH = 2000;
const IMAGE_FALLBACK_WIDTH = 320;
const IMAGE_KEY_STEP = 10;
const IMAGE_KEY_LARGE_STEP = 50;

export type ComposeBodyEditorHandle = {
  /** Serialize only if the browser-owned DOM has changed. */
  captureChanges(): Pick<Draft, "body" | "bodyHtml" | "signature" | "forwardedEdit"> | null;
  /** Whether only the original generated history remains. */
  isUntouchedEmpty(): boolean;
  replaceSignature(signature: DraftSignature | null | undefined): void;
  insertText(text: string): void;
  prependText(text: string): void;
  focusBody(): void;
  reviewBody(): { html: string; text: string; hasInlineImages: boolean; hasInlineQuotes: boolean };
  replaceAuthoredHtml(html: string): void;
};

type ComposeBodyEditorProps = {
  initial: Draft;
  attachments: Attachment[];
  messageAppearance?: MessageAppearance;
  busy: boolean;
  recipientTo: string;
  availabilityText: string | null;
  expandQuotedText?: boolean;
  writingTools?: ReactNode;
  snippets: Snippet[];
  onCreateSnippet(name: string, body: string): Promise<Snippet>;
  onUpdateSnippet(id: string, name: string, body: string): Promise<Snippet>;
  onDeleteSnippet(id: string): Promise<void>;
  onChange(): void;
  /** Called when a non-empty selection inside the editable text starts or ends. */
  onSelectionChange?(hasSelection: boolean): void;
  onError(message: string): void;
  onAttachImage(file: File, data: string): Promise<Attachment | undefined>;
  onRemoveImage(attachmentId: string): void;
  readInlineImage(attachmentId: string): Promise<string>;
};

/** Owns editable DOM, selection, and compose-only controls; persistence stays outside. */
export const ComposeBodyEditor = forwardRef<ComposeBodyEditorHandle, ComposeBodyEditorProps>(function ComposeBodyEditor({
  initial, attachments, messageAppearance, busy, recipientTo, availabilityText, expandQuotedText = false, snippets, writingTools,
  onCreateSnippet, onUpdateSnippet, onDeleteSnippet,
  onChange, onSelectionChange, onError, onAttachImage, onRemoveImage, readInlineImage,
}, ref) {
  const bodyEditor = useRef<HTMLDivElement>(null);
  const quotedEditor = useRef<HTMLDivElement>(null);
  const signatureEditor = useRef<HTMLDivElement>(null);
  const forwardedEditor = useRef<HTMLDivElement>(null);
  // The editable copy of a forward's sender HTML, or null while the original
  // shows read-only. The ref lets a save read it before the editor mounts.
  const [forwardedEdit, setForwardedEditState] = useState(() => initial.forwardedContent && initial.forwardedEdit ? sanitizeComposeHtml(initial.forwardedEdit.html) : null);
  const forwardedEditHtml = useRef(forwardedEdit);
  const [confirmingRestore, setConfirmingRestore] = useState(false);
  const [signatureState, setSignatureState] = useState(initial.signature);
  const signatureSnapshot = useRef(initial.signature);
  const signatureMarkup = useMemo(() => signatureState?.html ? signatureEditorHtml(signatureState.html, true) : "", [signatureState]);
  const bodyDirty = useRef(false);
  const mounted = useRef(true);
  // Restore the caret after focus moves to the context panel.
  const lastBodyRange = useRef<Range | null>(null);
  const savedSnippetRange = useRef<Range | null>(null);
  const insertedAvailabilityText = useRef<string | null>(null);
  const [snippetPickerOpen, setSnippetPickerOpen] = useState(false);
  useEffect(() => {
    if (!onSelectionChange) return;
    let selected = false;
    const update = () => {
      const selection = document.getSelection();
      const next = Boolean(selection && !selection.isCollapsed && selection.rangeCount
        && [bodyEditor.current, quotedEditor.current, signatureEditor.current, forwardedEditor.current].some((editor) => editor?.contains(selection.getRangeAt(0).commonAncestorContainer)));
      if (next === selected) return;
      selected = next;
      onSelectionChange(next);
    };
    document.addEventListener("selectionchange", update);
    return () => {
      document.removeEventListener("selectionchange", update);
      if (selected) onSelectionChange(false);
    };
  }, [onSelectionChange]);
  const [quoteExpanded, setQuoteExpanded] = useState(expandQuotedText);
  // Lazy initializer: `useRef(expr)` would evaluate `expr` on every render, so
  // each status change re-sanitized the whole quoted thread before the next paint.
  const [initialBody] = useState(() => {
    const rawHtml = initial.bodyHtml || draftTextToComposeHtml(initial.body);
    const boundary = initial.signature?.htmlOffset;
    const separator = "<br><br>";
    // This is the app-owned boundary captured with the draft, not an
    // inference from a sender's markup. It also preserves plain forwarded
    // history, which has no citation element, after a draft is reopened.
    if (initial.mode !== "new" && initial.bodyHtml && boundary !== undefined
      && Number.isInteger(boundary) && boundary >= 0 && rawHtml.slice(boundary).startsWith(separator)) {
      return { authoredHtml: sanitizeComposeHtml(rawHtml.slice(0, boundary)), quotedHtml: sanitizeComposeHtml(rawHtml.slice(boundary + separator.length)) };
    }
    const html = sanitizeComposeHtml(rawHtml);
    if (initial.mode === "forward" && initial.body.startsWith("\n\n---------- Forwarded message")) return { authoredHtml: "", quotedHtml: html };
    return splitReplyQuote(html) ?? { authoredHtml: html, quotedHtml: null };
  });

  const resolveForwardedImage = useCallback((url: string) => {
    if (!/^cid:/i.test(url)) return mailClient.fetchRemoteImage(url);
    const attachment = attachments.find((candidate) => candidate.inline
      && candidate.contentId && normalizeContentId(candidate.contentId) === normalizeContentId(url.slice(4)));
    return attachment ? readInlineImage(attachment.id) : Promise.reject(new Error("Embedded image not found"));
  }, [attachments, readInlineImage]);

  const editBody = useCallback(() => {
    // Keep cloning and sanitization off the keystroke path, including long quotes.
    bodyDirty.current = true;
    if (signatureEditor.current?.contains(document.activeElement) && signatureSnapshot.current) signatureSnapshot.current = { ...signatureSnapshot.current, automatic: false };
    onChange();
  }, [onChange]);
  // A link or color shortcut asks for its value in-app, then applies it to the
  // saved selection once the dialog has closed and the editor is no longer inert.
  type FormattingPrompt = { shortcut: FormattingShortcut; editor: HTMLElement; range: Range | null };
  const [formattingPrompt, setFormattingPrompt] = useState<FormattingPrompt | null>(null);
  const finishedPrompt = useRef<(FormattingPrompt & { value: string | null }) | null>(null);
  useEffect(() => {
    const finished = finishedPrompt.current;
    if (formattingPrompt || !finished) return;
    finishedPrompt.current = null;
    finished.editor.focus();
    if (finished.range) {
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(finished.range);
    }
    if (finished.value === null) return;
    applyFormattingShortcut(finished.editor, finished.shortcut, () => finished.value);
    editBody();
  }, [formattingPrompt, editBody]);
  const captureChanges = useCallback(() => {
    if (!bodyDirty.current || !bodyEditor.current) return null;
    const current = signatureSnapshot.current;
    let htmlOffset = 0, textOffset = 0;
    const { html, text } = serializeComposeBody(bodyEditor.current, quotedEditor.current, current ? (authored) => { htmlOffset = authored.html.length; textOffset = authored.text.length; } : undefined);
    const content = current && signatureEditor.current ? readSignatureEditor(signatureEditor.current) : current;
    // Preserve the saved plain-text alternative while editing the message.
    if (current && content && (current.html === content.html || current.text !== readSignatureHtml(current.html, true).text)) content.text = current.text;
    const signature = current && content ? { ...current, html: content.html, text: content.text,
      automatic: current.automatic && current.html === content.html && current.text === content.text,
      htmlOffset, textOffset,
    } : current;
    signatureSnapshot.current = signature;
    bodyDirty.current = false;
    if (!initial.forwardedContent) return { body: text, bodyHtml: html, signature };
    const editedHtml = forwardedEditHtml.current;
    const forwardedSource = editedHtml === null ? null : forwardedEditor.current ?? Object.assign(document.createElement("div"), { innerHTML: editedHtml });
    return { body: text, bodyHtml: html, signature, forwardedEdit: forwardedSource ? serializeComposeBody(forwardedSource) : null };
  }, [initial.forwardedContent]);
  const setForwardedEdit = useCallback((html: string | null) => {
    forwardedEditHtml.current = html;
    setForwardedEditState(html);
    setConfirmingRestore(false);
    editBody();
  }, [editBody]);
  const prependText = useCallback((text: string) => {
    if (!bodyEditor.current) return;
    // Availability and AI output are text; escape markup before insertion.
    bodyEditor.current.insertAdjacentHTML("afterbegin", sanitizeComposeHtml(plainTextToHtml(text)));
    editBody();
  }, [editBody]);
  function insertText(text: string) {
    const editor = bodyEditor.current;
    if (!editor || busy) return;
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
  function insertPastedImage(editor: HTMLElement, file: File, range: Range | null) {
    const reader = new FileReader();
    reader.onerror = () => onError(`Could not paste ${file.name || "image"}.`);
    reader.onload = () => {
      if (!mounted.current || typeof reader.result !== "string" || !editor.isConnected) return;
      const preview = reader.result;
      const data = preview.slice(preview.indexOf(",") + 1);
      void (async () => {
        const attachment = await onAttachImage(file, data);
        if (!attachment || !mounted.current || !editor.isConnected) return;

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
      })().catch((reason) => onError(errorMessage(reason)));
    };
    reader.readAsDataURL(file);
  }
  useImperativeHandle(ref, () => ({
    captureChanges, insertText, prependText, focusBody,
    isUntouchedEmpty: () => {
      if (!bodyEditor.current || initial.forwardedEdit || forwardedEditHtml.current !== null) return false;
      const original = document.createElement("div");
      original.innerHTML = initialBody.authoredHtml;
      const empty = (element: HTMLElement) => !element.textContent?.trim() && !element.querySelector("img");
      if (!empty(original) || !empty(bodyEditor.current)) return false;
      original.innerHTML = initialBody.quotedHtml ?? "";
      return serializeComposeBody(original).html === (quotedEditor.current ? serializeComposeBody(quotedEditor.current).html : "");
    },
    replaceSignature: (signature) => { signatureSnapshot.current = signature; setSignatureState(signature); editBody(); },
    reviewBody: () => {
      if (!bodyEditor.current) throw new Error("The draft editor is unavailable");
      return {
        ...serializeComposeBody(bodyEditor.current),
        hasInlineImages: Boolean(bodyEditor.current.querySelector("img")),
        // A citation mixed into an inline answer cannot be replaced as one
        // authored paragraph without also changing somebody else's words.
        hasInlineQuotes: Boolean(bodyEditor.current.querySelector('blockquote[type="cite"]')),
      };
    },
    replaceAuthoredHtml: (html) => {
      if (!bodyEditor.current || busy) throw new Error("Finish the current composer action first");
      bodyEditor.current.innerHTML = sanitizeComposeHtml(html);
      lastBodyRange.current = null;
      editBody();
    },
  }));
  useEffect(() => {
    mounted.current = true;
    [bodyEditor.current, quotedEditor.current, signatureEditor.current, forwardedEditor.current].flatMap((editor) => Array.from(editor?.querySelectorAll<HTMLImageElement>("img") ?? [])).forEach((image) => {
      const source = image.getAttribute("src") ?? "";
      const attachment = source.startsWith("cid:")
        ? attachments.find((candidate) => candidate.inline && candidate.contentId === source.slice(4))
        : undefined;
      if (attachment) {
        image.dataset.composeSource = source;
        void readInlineImage(attachment.id).then((preview) => { image.src = preview; }).catch((reason) => onError(String(reason)));
      }
      // Removing a forwarded image keeps its attachment, so Restore Original still shows it.
      if (!signatureEditor.current?.contains(image)) decorateImage(image, forwardedEditor.current?.contains(image) ? undefined : attachment?.id);
    });
    return () => { mounted.current = false; };
  }, [attachments, onError, readInlineImage, signatureState, forwardedEdit]);
  useEffect(() => {
    const root = signatureEditor.current;
    if (!root) return;
    let active = true;
    // This is the user's chosen signature. Display its images automatically
    // through the same bounded native proxy; incoming mail stays blocked.
    void loadSignatureEditorImages(root).then((failures) => {
      if (active && failures.length) onError(`Could not load signature images: ${failures[0]}`);
    });
    return () => { active = false; };
  }, [signatureState, onError]);
  useEffect(() => {
    if (!availabilityText || insertedAvailabilityText.current === availabilityText) return;
    insertedAvailabilityText.current = availabilityText;
    prependText(`${availabilityText}\n\n`);
  }, [availabilityText, prependText]);
  useEffect(() => {
    // Capture the displayed authored/quoted boundary even when a mailto or
    // other prefill is sent without typing. Native-created snapshots start at
    // zero; the UI owns the eventual writing boundary and sanitized content.
    if (signatureSnapshot.current) editBody();
  }, [editBody, signatureState]);
  // Editing behavior shared by the body, a reply's quoted history, and an edited forward.
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
      try {
        const linkSelection = href && selection && !selection.isCollapsed && selection.toString().trim() && event.currentTarget.contains(selection.anchorNode);
        // Keep browser undo history, but do not depend on execCommand emitting
        // an input event: some webviews apply the edit without notifying us.
        const applied = linkSelection
          ? document.execCommand("createLink", false, href)
          : document.execCommand("insertHTML", false, linkifyPlainText(text));
        if (!applied) throw new Error("The editor could not apply the paste.");
        editBody();
      } catch (reason) {
        onError(`Could not paste: ${errorMessage(reason)}`);
      }
    },
    onClick: (event: ReactMouseEvent<HTMLDivElement>) => {
      const remove = closestFrom<HTMLElement>(event.target, "[data-compose-image-remove]");
      if (!remove) return;
      const wrapper = remove.closest<HTMLElement>("[data-compose-image]");
      const attachmentId = wrapper?.dataset.attachmentId;
      wrapper?.remove();
      editBody();
      event.currentTarget.focus();
      if (attachmentId) onRemoveImage(attachmentId);
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
      if (!event.nativeEvent.isComposing && event.key === "Enter" && !event.shiftKey && !event.altKey && !event.metaKey && !event.ctrlKey
        && applyCodeBlockEnter(event.currentTarget)) {
        event.preventDefault();
        event.stopPropagation();
        editBody();
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
      if (shortcut.prompt) {
        event.preventDefault();
        event.stopPropagation();
        const selection = window.getSelection();
        const range = selection?.rangeCount && event.currentTarget.contains(selection.anchorNode) ? selection.getRangeAt(0).cloneRange() : null;
        setFormattingPrompt({ shortcut, editor: event.currentTarget, range });
        return;
      }
      if (applyFormattingShortcut(event.currentTarget, shortcut)) {
        event.preventDefault();
        event.stopPropagation();
        editBody();
      }
    },
  };
  return <>
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
    {signatureState?.html ? <div key={`${signatureState.id}:${signatureState.html}`} ref={signatureEditor} className="compose-body compose-signature" role="textbox" aria-label="Message Signature" aria-multiline="true" tabIndex={0}
      dangerouslySetInnerHTML={{ __html: signatureMarkup }} {...editingProps}
      onInput={() => {
        if (signatureSnapshot.current) signatureSnapshot.current = { ...signatureSnapshot.current, automatic: false };
        editBody();
      }} onClick={(event) => { if ((event.target as Element).closest("a")) event.preventDefault(); editingProps.onClick(event); }} /> : null}
    {initialBody.quotedHtml !== null || writingTools ? <div className="compose-body-actions">
      {initialBody.quotedHtml !== null ? (
        <button
          type="button"
          className="btn btn-sm compose-quote-toggle"
          aria-label={quoteExpanded ? "Hide Quoted Text" : "Show Quoted Text"}
          aria-expanded={quoteExpanded}
          aria-controls={`${initial.id}-quoted`}
          title={quoteExpanded ? "Hide quoted text" : "Show quoted text"}
          onClick={() => setQuoteExpanded((expanded) => !expanded)}
        >···</button>
      ) : null}
      {writingTools}
    </div> : null}
    {/* Collapsed with `hidden`, so assistive and dictation software
        reading the page skip the history until it is shown. */}
    {initialBody.quotedHtml !== null ? (
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
    ) : null}
    {initial.forwardedContent ? (
      <section className="compose-forwarded" aria-label="Forwarded message">
        {/* Sender HTML shows read-only until the user asks to edit it; editing
            converts it once through the compose sanitizer. */}
        <div className="compose-forwarded-bar">
          <span>{forwardedEdit === null ? "Forwarded message" : "Editing forwarded message · Some formatting may be simplified"}</span>
          {forwardedEdit === null ? (
            <button type="button" className="btn btn-sm" disabled={busy} aria-label="Edit Forwarded Message"
              title="Edit the forwarded message. Some formatting and web images may be removed."
              onClick={() => setForwardedEdit(forwardedComposeHtml(initial.forwardedContent!.html))}>Edit</button>
          ) : (
            <button type="button" className="btn btn-sm" disabled={busy || confirmingRestore} onClick={() => setConfirmingRestore(true)}>Restore Original</button>
          )}
        </div>
        {confirmingRestore ? (
          <InlineConfirm ariaLabel="Restore original forwarded message confirmation" title="Restore the original message?" cancelLabel="Keep Changes"
            onCancel={() => setConfirmingRestore(false)} disabled={busy} focusCancel
            actions={[{ label: "Restore Original", className: "btn-danger", onClick: () => setForwardedEdit(null) }]}>
            Your changes to the forwarded message will be lost.
          </InlineConfirm>
        ) : null}
        {forwardedEdit === null ? (
          <SafeMessage {...messageAppearance} foldQuotes={false} html={initial.forwardedContent.html} text={initial.forwardedContent.text}
            resolveImage={resolveForwardedImage} imageCacheKey={`${initial.id}:${attachments.filter((attachment) => attachment.inline && attachment.ready).map((attachment) => attachment.id).join(",")}`} />
        ) : (
          <div
            ref={forwardedEditor}
            className="compose-body compose-forwarded-edit"
            role="textbox"
            aria-label="Forwarded Message"
            aria-multiline="true"
            aria-disabled={busy}
            dangerouslySetInnerHTML={{ __html: forwardedEdit }}
            {...editingProps}
          />
        )}
      </section>
    ) : null}
    {formattingPrompt?.shortcut.prompt ? (
      <FormattingPromptDialog
        kind={formattingPrompt.shortcut.prompt}
        onClose={() => {
          finishedPrompt.current = { ...formattingPrompt, value: null };
          setFormattingPrompt(null);
        }}
        onSubmit={(value) => {
          finishedPrompt.current = { ...formattingPrompt, value };
          setFormattingPrompt(null);
        }}
      />
    ) : null}
    {snippetPickerOpen ? (
      <SnippetPicker
        snippets={snippets}
        onClose={() => setSnippetPickerOpen(false)}
        onInsert={(snippet) => {
          recordSnippetUsed(snippet.id);
          setSnippetPickerOpen(false);
          const saved = savedSnippetRange.current;
          const editor = [bodyEditor.current, quotedEditor.current, forwardedEditor.current].find((candidate) => candidate && saved && candidate.contains(saved.startContainer)) ?? bodyEditor.current;
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
          const rendered = renderSnippetBody(snippet.body, { firstName: firstNameFromRecipient(recipientTo) });
          insertHtmlAtRange(editor, range, rendered);
          editBody();
        }}
        onCreate={onCreateSnippet}
        onUpdate={onUpdateSnippet}
        onDelete={onDeleteSnippet}
      />
    ) : null}
  </>;
});

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
