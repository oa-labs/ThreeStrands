import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { Paperclip, Send, X, Trash2 } from "lucide-react";
import { mailClient } from "./data/client";
import type { Draft, OutboxItem } from "./correspondence";
import {
  clampComposerPosition,
  clampComposerSize,
  minimumComposerHeight,
  minimumComposerWidth,
  readComposerPosition,
  readComposerSize,
  saveComposerPosition,
  saveComposerSize,
  type ComposerPosition,
  type ComposerSize,
} from "./composerLayout";
import { useEscapeDismiss } from "./useEscapeDismiss";

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
  const [preferredSize, setPreferredSize] = useState(readComposerSize);
  const [preferredPosition, setPreferredPosition] = useState(readComposerPosition);
  const [measuredSize, setMeasuredSize] = useState<ComposerSize | null>(null);
  const [moving, setMoving] = useState(false);
  const [viewportSize, setViewportSize] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }));
  const panel = useRef<HTMLDivElement>(null);
  const moveDrag = useRef<{ x: number; y: number; position: ComposerPosition; pointerId: number } | null>(null);
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
  useEffect(() => {
    const onResize = () => setViewportSize({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  useEscapeDismiss(close);

  const visibleSize = preferredSize ? clampComposerSize(preferredSize, viewportSize) : null;
  useLayoutEffect(() => {
    const rect = panel.current?.getBoundingClientRect();
    if (!rect) return;
    setMeasuredSize((current) => (
      current && Math.round(current.width) === Math.round(rect.width) && Math.round(current.height) === Math.round(rect.height)
        ? current
        : { width: rect.width, height: rect.height }
    ));
  });
  const layoutSize = visibleSize ?? measuredSize ?? { width: minimumComposerWidth, height: minimumComposerHeight };
  const visiblePosition = preferredPosition ? clampComposerPosition(preferredPosition, layoutSize, viewportSize) : null;
  const resize = (size: ComposerSize) => {
    const next = clampComposerSize(size, viewportSize);
    setPreferredSize(next);
    saveComposerSize(next);
    if (preferredPosition) {
      const nextPosition = clampComposerPosition(preferredPosition, next, viewportSize);
      setPreferredPosition(nextPosition);
      saveComposerPosition(nextPosition);
    }
  };
  const move = (position: ComposerPosition) => {
    const next = clampComposerPosition(position, layoutSize, viewportSize);
    setPreferredPosition(next);
    saveComposerPosition(next);
  };
  function onHeaderPointerDown(event: PointerEvent<HTMLElement>) {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest("button")) return;
    event.preventDefault();
    const rect = panel.current?.getBoundingClientRect();
    if (!rect) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    moveDrag.current = { x: event.clientX, y: event.clientY, position: { x: rect.left, y: rect.top }, pointerId: event.pointerId };
    setMoving(true);
  }
  function onHeaderPointerMove(event: PointerEvent<HTMLElement>) {
    if (moveDrag.current?.pointerId !== event.pointerId) return;
    move({
      x: moveDrag.current.position.x + event.clientX - moveDrag.current.x,
      y: moveDrag.current.position.y + event.clientY - moveDrag.current.y,
    });
  }
  function endHeaderDrag(event: PointerEvent<HTMLElement>) {
    if (moveDrag.current?.pointerId !== event.pointerId) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    moveDrag.current = null;
    setMoving(false);
  }

  const frameStyle = {
    ...(visibleSize ? { width: visibleSize.width, height: visibleSize.height } : {}),
    ...(visiblePosition ? { left: visiblePosition.x, top: visiblePosition.y } : {}),
  } as CSSProperties;

  return <div className="compose-backdrop">
    <div ref={panel} className={`composer${visibleSize ? " sized" : ""}${visiblePosition ? " placed" : ""}`} role="dialog" aria-modal="true" aria-label={initial.mode === "new" ? "New message" : initial.mode === "forward" ? "Forward message" : "Reply message"}
      style={Object.keys(frameStyle).length ? frameStyle : undefined}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Tab") {
          const controls = Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), [tabindex="0"]') ?? []);
          const first = controls[0], last = controls.at(-1);
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
      <header
        className={`composer-header${moving ? " dragging" : ""}`}
        title="Drag to move"
        onPointerDown={onHeaderPointerDown}
        onPointerMove={onHeaderPointerMove}
        onPointerUp={endHeaderDrag}
        onLostPointerCapture={() => { moveDrag.current = null; setMoving(false); }}
      ><div><h2>{initial.mode === "new" ? "New message" : initial.mode === "forward" ? "Forward" : initial.mode === "replyAll" ? "Reply all" : "Reply"}</h2><span>From {draft.account}</span></div><button className="icon-button" aria-label="Save and close draft" onClick={close} disabled={busy}><X size={19} /></button></header>
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
      <ComposerResizeHandle
        panel={panel}
        size={visibleSize}
        onResize={resize}
      />
    </div>
  </div>;
});

function ComposerResizeHandle({
  panel,
  size,
  onResize,
}: {
  panel: React.RefObject<HTMLDivElement | null>;
  size: ComposerSize | null;
  onResize(size: ComposerSize): void;
}) {
  const drag = useRef<{ x: number; y: number; size: ComposerSize; pointerId: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const currentSize = () => size ?? {
    width: panel.current?.getBoundingClientRect().width ?? minimumComposerWidth,
    height: panel.current?.getBoundingClientRect().height ?? minimumComposerHeight,
  };

  return <div
    className={`composer-resizer${dragging ? " dragging" : ""}`}
    role="button"
    tabIndex={0}
    aria-label="Resize compose window"
    title="Drag to resize. Use arrow keys to adjust."
    onPointerDown={(event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.focus();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { x: event.clientX, y: event.clientY, size: currentSize(), pointerId: event.pointerId };
      setDragging(true);
    }}
    onPointerMove={(event) => {
      if (drag.current?.pointerId === event.pointerId) {
        onResize({
          width: drag.current.size.width + event.clientX - drag.current.x,
          height: drag.current.size.height + event.clientY - drag.current.y,
        });
      }
    }}
    onPointerUp={(event) => {
      if (drag.current?.pointerId === event.pointerId) {
        event.currentTarget.releasePointerCapture(event.pointerId);
        drag.current = null;
        setDragging(false);
      }
    }}
    onLostPointerCapture={() => {
      drag.current = null;
      setDragging(false);
    }}
    onKeyDown={(event) => {
      const step = event.shiftKey ? 40 : 10;
      const current = currentSize();
      const next = event.key === "ArrowLeft" ? { ...current, width: current.width - step }
        : event.key === "ArrowRight" ? { ...current, width: current.width + step }
        : event.key === "ArrowUp" ? { ...current, height: current.height - step }
        : event.key === "ArrowDown" ? { ...current, height: current.height + step }
        : null;
      if (next) {
        event.preventDefault();
        event.stopPropagation();
        onResize(next);
      }
    }}
  />;
}
