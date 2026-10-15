import { useEffect, useRef, useState } from "react";
import { Modal } from "./AppChrome";
import { SafeMessage } from "./SafeMessage";
import { FormattingPromptDialog } from "./FormattingPromptDialog";
import { applyFormattingShortcut, formattingShortcutFor, formattingShortcuts, insertHtmlAtRange, linkifyPlainText } from "./richText";
import { readSignatureEditor, sanitizeSignatureHtml, validateSignature, type Signature } from "./signatures";
import { sanitizeHtmlDimension, SIGNATURE_LIMITS } from "./emailRenderingPolicy";
import { errorMessage } from "./errors";
import { remoteImageSources, embedRemoteImages } from "./signatureImport";

export function SignatureEditor({ signature, onSave, onClose }: {
  signature: Signature | null; onSave(signature: Signature): Promise<void>; onClose(): void;
}) {
  const [name, setName] = useState(signature?.name ?? "");
  const [signatureId] = useState(() => signature?.id ?? crypto.randomUUID());
  const [initialHtml] = useState(() => sanitizeSignatureHtml(signature?.html ?? ""));
  const editor = useRef<HTMLDivElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  const lastRange = useRef<Range | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [status, setStatus] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const [imageTarget, setImageTarget] = useState<HTMLImageElement | "new" | null>(null);
  const [imageChoices, setImageChoices] = useState<HTMLImageElement[] | null>(null);
  const [prompt, setPrompt] = useState<"link" | "color" | null>(null);
  const finishedPrompt = useRef<{ kind: "link" | "color"; value: string } | null>(null);

  function rememberRange() {
    const selection = window.getSelection();
    if (selection?.rangeCount && editor.current?.contains(selection.anchorNode)) lastRange.current = selection.getRangeAt(0).cloneRange();
  }
  function restoreRange() {
    const root = editor.current!;
    root.focus();
    const range = lastRange.current && root.contains(lastRange.current.commonAncestorContainer) ? lastRange.current : document.createRange();
    if (range !== lastRange.current) { range.selectNodeContents(root); range.collapse(false); }
    window.getSelection()?.removeAllRanges(); window.getSelection()?.addRange(range);
    return range;
  }
  function command(command: string, value?: string) {
    restoreRange();
    document.execCommand(command, false, value);
    rememberRange();
  }
  useEffect(() => {
    const finished = finishedPrompt.current;
    if (prompt || !finished || !editor.current) return;
    finishedPrompt.current = null;
    restoreRange();
    const shortcut = formattingShortcuts.find((s) => s.prompt === finished.kind)!;
    applyFormattingShortcut(editor.current, shortcut, () => finished.value);
  });

  async function save() {
    if (busy || !editor.current) return;
    setError("");
    try {
      const content = readSignatureEditor(editor.current);
      const next = { id: signatureId, name: name.trim(), ...content };
      validateSignature(next);
      setBusy(true);
      await onSave(next);
      onClose();
    } catch (reason) { setError(errorMessage(reason)); setBusy(false); }
  }

  return <Modal title={signature ? "Edit Signature" : "New Signature"} initialFocusRef={nameInput} onClose={() => { if (!busy) onClose(); }} className="signature-dialog">
    <form className="signature-form" onSubmit={(event) => { event.preventDefault(); void save(); }} onKeyDown={(event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void save(); }
    }}>
      <label className="snippet-field"><span>Name</span><input ref={nameInput} autoFocus value={name} disabled={busy} onChange={(event) => setName(event.target.value)} /></label>
      <div className="signature-toolbar" role="group" aria-label="Signature Formatting">
        {[['bold', 'Bold'], ['italic', 'Italic'], ['underline', 'Underline']].map(([action, label]) => <button key={action} className="btn btn-sm" type="button" disabled={busy} onClick={() => command(action)}>{label}</button>)}
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => setPrompt("link")}>Link…</button>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => setPrompt("color")}>Text Color…</button>
        <select aria-label="Signature Font" defaultValue="" disabled={busy} onChange={(event) => { command("fontName", event.target.value); event.target.value = ""; }}>
          <option value="" disabled>Font</option><option value="sans-serif">Sans Serif</option><option value="serif">Serif</option><option value="monospace">Fixed-Width</option>
        </select>
        <select aria-label="Signature Text Size" defaultValue="" disabled={busy} onChange={(event) => { command("fontSize", event.target.value); event.target.value = ""; }}>
          <option value="" disabled>Text Size</option><option value="2">Small</option><option value="3">Medium</option><option value="4">Large</option>
        </select>
        <select aria-label="Signature Alignment" defaultValue="" disabled={busy} onChange={(event) => { command(event.target.value); event.target.value = ""; }}>
          <option value="" disabled>Alignment</option><option value="justifyLeft">Left</option><option value="justifyCenter">Center</option><option value="justifyRight">Right</option>
        </select>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => setImageTarget("new")}>Add Image…</button>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => {
          const images = Array.from(editor.current?.querySelectorAll("img") ?? []);
          if (images.length) setImageChoices(images); else setError("Add an image before editing its size or description.");
        }}>Edit Image…</button>
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => setImportOpen(true)}>Import…</button>
      </div>
      <div ref={editor} className="signature-content" role="textbox" aria-label="Signature Content" aria-multiline="true" tabIndex={0}
        contentEditable={!busy} suppressContentEditableWarning dangerouslySetInnerHTML={{ __html: initialHtml }}
        onBlur={rememberRange} onKeyUp={rememberRange} onMouseUp={rememberRange}
        onClick={(event) => { const image = (event.target as Element).closest("img"); if (image) { event.preventDefault(); setImageTarget(image); } if ((event.target as Element).closest("a")) event.preventDefault(); }}
        onKeyDown={(event) => {
          const shortcut = formattingShortcutFor(event.nativeEvent);
          if (!shortcut) return;
          if (shortcut.prompt) { event.preventDefault(); event.stopPropagation(); rememberRange(); setPrompt(shortcut.prompt); }
          else if (applyFormattingShortcut(event.currentTarget, shortcut)) { event.preventDefault(); event.stopPropagation(); }
        }}
        onPaste={(event) => {
          event.preventDefault();
          rememberRange();
          const image = Array.from(event.clipboardData.files).find((file) => file.type.startsWith("image/"));
          if (image) {
            void readImageFile(image).then((source) => {
              if (!editor.current) return;
              const element = document.createElement("img"); element.src = source; element.alt = image.name || "Signature image";
              element.width = SIGNATURE_LIMITS.defaultImageWidth;
              insertHtmlAtRange(editor.current, restoreRange(), element.outerHTML);
              rememberRange();
            }).catch((reason) => setError(errorMessage(reason)));
            return;
          }
          const html = event.clipboardData.getData("text/html");
          if (!html || !remoteImageSources(html).length) {
            insertHtmlAtRange(event.currentTarget, restoreRange(), html ? sanitizeSignatureHtml(html) : linkifyPlainText(event.clipboardData.getData("text/plain")));
            rememberRange();
            return;
          }
          // Signatures copied from webmail link their logos; download them so
          // the saved signature carries its own images, as an upload would.
          setStatus("Downloading images…");
          void embedRemoteImages(html).then(({ html: embedded, failures }) => {
            setStatus("");
            if (!editor.current) return;
            insertHtmlAtRange(editor.current, restoreRange(), sanitizeSignatureHtml(embedded));
            rememberRange();
            if (!failures.length) return;
            const reason = errorMessage(failures[0]);
            setError(`${failures.length === 1 ? "An image" : `${failures.length} images`} couldn't be added. ${reason}${/[.!?]$/.test(reason) ? "" : "."} Use Add Image… to add ${failures.length === 1 ? "it" : "them"} from your computer.`);
          });
        }} />
      <p className="settings-hint">Paste your existing signature or write one here. Select text to format it; click an image to edit its size and description.</p>
      {status ? <p className="settings-hint" role="status">{status}</p> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="signature-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button><button type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Saving…" : "Save"}</button></div>
    </form>
    {prompt ? <FormattingPromptDialog kind={prompt} onClose={() => setPrompt(null)} onSubmit={(value) => { finishedPrompt.current = { kind: prompt, value }; setPrompt(null); }} /> : null}
    {importOpen ? <SignatureImportDialog onClose={() => setImportOpen(false)} onImport={(html) => { editor.current!.innerHTML = html; lastRange.current = null; setError(""); setImportOpen(false); }} /> : null}
    {imageChoices ? <Modal title="Signature Images" onClose={() => setImageChoices(null)}><div className="signature-form">{imageChoices.map((image, index) => <button type="button" className="btn" key={index} onClick={() => { setImageTarget(image); setImageChoices(null); }}>Edit {image.alt || `Image ${index + 1}`}</button>)}<div className="signature-actions"><button type="button" className="btn" onClick={() => setImageChoices(null)}>Cancel</button></div></div></Modal> : null}
    {imageTarget ? <SignatureImageDialog image={imageTarget === "new" ? null : imageTarget} onClose={() => setImageTarget(null)} onSave={(source, alt, width) => {
      const image = imageTarget === "new" ? document.createElement("img") : imageTarget;
      image.src = source; image.alt = alt;
      image.setAttribute("width", width);
      if (imageTarget === "new") insertHtmlAtRange(editor.current!, restoreRange(), image.outerHTML);
      setImageTarget(null);
    }} onRemove={imageTarget === "new" ? undefined : () => { imageTarget.remove(); setImageTarget(null); }} /> : null}
  </Modal>;
}

export function SignatureImportDialog({ onClose, onImport }: { onClose(): void; onImport(html: string): void }) {
  const sourceInput = useRef<HTMLTextAreaElement>(null);
  const [source, setSource] = useState("");
  const [embedRemote, setEmbedRemote] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const remote = remoteImageSources(source);
  async function importHtml() {
    setBusy(true); setError("");
    try {
      let embedded = source;
      if (embedRemote) {
        const result = await embedRemoteImages(source);
        if (result.failures.length) throw result.failures[0];
        embedded = result.html;
      }
      const html = sanitizeSignatureHtml(embedded);
      const node = document.createElement("div"); node.innerHTML = html;
      validateSignature({ id: "preview", name: "Imported", ...readSignatureEditor(node) });
      onImport(html);
    } catch (reason) { setError(errorMessage(reason)); setBusy(false); }
  }
  return <Modal title="Import Signature" initialFocusRef={sourceInput} onClose={() => { if (!busy) onClose(); }} className="signature-dialog">
    <form className="signature-form" onSubmit={(event) => { event.preventDefault(); void importHtml(); }}>
      <label className="snippet-field"><span>HTML</span><textarea ref={sourceInput} autoFocus rows={6} value={source} disabled={busy} onChange={(event) => setSource(event.target.value)} placeholder="Paste your signature HTML" /></label>
      <label className="snippet-field"><span>Signature File</span><input type="file" accept=".html,.htm,.txt" disabled={busy} onChange={(event) => {
        const file = event.target.files?.[0]; if (!file) return;
        if (file.size > SIGNATURE_LIMITS.maxHtmlBytes) { setError("This signature file is too large."); return; }
        void file.text().then((text) => setSource(file.name.toLowerCase().endsWith(".txt") ? linkifyPlainText(text) : text)).catch((reason) => setError(errorMessage(reason)));
      }} /></label>
      {remote.length ? <label className="settings-checkbox"><input type="checkbox" checked={embedRemote} disabled={busy} onChange={(event) => setEmbedRemote(event.target.checked)} /><span>Embed Remote Images</span></label> : null}
      {remote.length ? <p className="settings-hint">{remote.length} remote image source{remote.length === 1 ? "" : "s"}. Embedding downloads images through ThreeStrands and saves them with the signature; otherwise they are left out.</p> : null}
      {source ? <section aria-label="Signature Preview"><h3>Preview</h3><SafeMessage html={source} text="" loadImages={false} foldQuotes={false} /></section> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="signature-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Cancel</button><button type="submit" className="btn btn-primary" disabled={busy || !source.trim()}>{busy ? "Importing…" : "Import"}</button></div>
    </form>
  </Modal>;
}

async function readImageFile(file: File): Promise<string> {
  if (!/^image\/(avif|gif|jpeg|png|webp)$/.test(file.type)) throw new Error("Choose a PNG, JPEG, GIF, WebP, or AVIF image.");
  if (file.size > SIGNATURE_LIMITS.maxImageBytes) throw new Error("This image is too large. Choose an image smaller than 128 KiB.");
  return new Promise((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(new Error("Could not read the image.")); reader.readAsDataURL(file); });
}

function SignatureImageDialog({ image, onClose, onSave, onRemove }: { image: HTMLImageElement | null; onClose(): void; onSave(source: string, alt: string, width: string): void; onRemove?: () => void }) {
  const imageInput = useRef<HTMLInputElement>(null);
  const [source, setSource] = useState(image?.getAttribute("src") ?? "");
  const [alt, setAlt] = useState(image?.alt ?? "");
  const [width, setWidth] = useState(image?.getAttribute("width") ?? String(SIGNATURE_LIMITS.defaultImageWidth));
  const [error, setError] = useState("");
  return <Modal title={image ? "Edit Signature Image" : "Add Signature Image"} initialFocusRef={imageInput} onClose={onClose}>
    <form className="signature-form" onSubmit={(event) => {
      event.preventDefault(); const size = sanitizeHtmlDimension(width, false);
      if (!source || !size || Number(size) <= 0) { setError("Choose an image and enter a valid width in pixels."); return; }
      onSave(source, alt, size);
    }}>
      <label className="snippet-field"><span>Image</span><input ref={imageInput} autoFocus type="file" accept="image/png,image/jpeg,image/gif,image/webp,image/avif" onChange={(event) => { const file = event.target.files?.[0]; if (file) void readImageFile(file).then((data) => { setSource(data); if (!alt) setAlt(file.name); }).catch((reason) => setError(errorMessage(reason))); }} /></label>
      <label className="snippet-field"><span>Description</span><input value={alt} onChange={(event) => setAlt(event.target.value)} /></label>
      <label className="snippet-field"><span>Width in Pixels</span><input inputMode="numeric" value={width} onChange={(event) => setWidth(event.target.value)} /></label>
      {source ? <img className="signature-image-preview" src={source} alt={alt} /> : null}
      {error ? <p className="form-error" role="alert">{error}</p> : null}
      <div className="signature-actions">{onRemove ? <button className="btn btn-danger" type="button" onClick={onRemove}>Remove Image</button> : null}<button className="btn" type="button" onClick={onClose}>Cancel</button><button className="btn btn-primary" type="submit">Apply</button></div>
    </form>
  </Modal>;
}
