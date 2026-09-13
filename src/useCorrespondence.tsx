import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { Composer, type ComposerHandle } from "./Composer";
import { mailClient } from "./data/client";
import type { ComposeMode, Draft, OutboxItem } from "./correspondence";
import type { Account } from "./domain";

export function useCorrespondence(accounts: Account[], sourceId?: string, sourceAccountId?: string) {
  const [active, setActive] = useState<Draft | null>(null);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [outbox, setOutbox] = useState<OutboxItem[]>([]);
  const [error, setError] = useState("");
  const [closing, setClosing] = useState(false);
  const editor = useRef<ComposerHandle>(null);
  const opening = useRef(false);
  const quitting = useRef(false);
  const [clock, setClock] = useState(Date.now());
  const refreshing = useRef(false);
  const refresh = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try { const [d, o] = await Promise.all([mailClient.listDrafts(), mailClient.listOutbox()]); setDrafts(d); setOutbox(o); }
    finally { refreshing.current = false; }
  }, []);
  useEffect(() => { void refresh().catch((e) => setError(String(e))); }, [refresh]);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const listener = listen("compose-before-exit", async () => {
      if (quitting.current) return;
      quitting.current = true; setClosing(true);
      try { await editor.current?.prepareExit(); await invoke("finish_exit"); }
      catch (e) { setError(`Could not close safely: ${String(e)}`); quitting.current = false; setClosing(false); }
    });
    return () => { void listener.then((unlisten) => unlisten()); };
  }, []);
  const start = useCallback(async (mode: ComposeMode) => {
    if (opening.current) return;
    opening.current = true;
    try {
      await editor.current?.flush();
      const d = mode === "new"
        ? await mailClient.createDraft(mode)
        : await mailClient.createDraft(mode, sourceId, sourceAccountId);
      setActive(d); setError("");
    }
    catch (e) { setError(String(e)); }
    finally { opening.current = false; }
  }, [sourceId, sourceAccountId]);
  // Called when navigating to the inline Drafts/Outbox view: makes sure
  // whatever was being edited is saved and the lists are current.
  const openList = useCallback(async () => {
    try { await editor.current?.flush(); setActive(null); await refresh(); }
    catch (e) { setError(String(e)); }
  }, [refresh]);
  const undo = useCallback(async (id?: string) => {
    const target = id ?? outbox.find((o) => ["undo_pending", "ready"].includes(o.state))?.id;
    if (!target) return;
    try { await editor.current?.flush(); const d = await mailClient.cancelSend(target); setActive(d); await refresh(); }
    catch (e) { setError(String(e)); }
  }, [outbox, refresh]);
  const restoreFailedSend = useCallback((id: string) => {
    void mailClient.recoverSend(id).then((d) => { setActive(d); void refresh(); }).catch((e: unknown) => setError(String(e)));
  }, [refresh]);
  const reconcileSend = useCallback((id: string) => {
    void mailClient.reconcileSend(id).then(refresh).catch((e: unknown) => setError(String(e)));
  }, [refresh]);
  useEffect(() => { if (closing) document.querySelector<HTMLElement>(".exit-notice")?.focus(); }, [closing]);
  const pending = outbox.find((o) => ["undo_pending", "ready"].includes(o.state));
  const pendingId = pending?.id ?? null;
  const hasActiveDelivery = outbox.some((o) => ["undo_pending", "ready", "sending"].includes(o.state));
  useEffect(() => {
    if (!hasActiveDelivery) return;
    const timer = window.setInterval(() => {
      setClock(Date.now());
      void refresh().catch(() => {});
    }, 1000);
    return () => window.clearInterval(timer);
  }, [hasActiveDelivery, refresh]);

  const compose = useCallback(() => { void start("new"); }, [start]);
  const reply = useCallback(() => { void start("reply"); }, [start]);
  const replyAll = useCallback(() => { void start("replyAll"); }, [start]);
  const forward = useCallback(() => { void start("forward"); }, [start]);
  const openInbox = useCallback(() => { setActive(null); }, []);
  const openDrafts = useCallback(() => { void openList(); }, [openList]);
  const openOutbox = useCallback(() => { void openList(); }, [openList]);
  const sendDraft = useCallback(() => editor.current?.send(), []);
  const attachFiles = useCallback(() => editor.current?.attach(), []);
  const undoSend = useCallback(() => { void undo(); }, [undo]);
  const composerActive = Boolean(active);

  const context = useMemo(() => ({
    closing,
    composerActive,
    compose, reply, replyAll, forward, openInbox, openDrafts, openOutbox,
    sendDraft, attachFiles, undoSend, canUndoSend: pendingId !== null,
  }), [attachFiles, closing, compose, composerActive, forward, openDrafts, openInbox, openOutbox, reply, replyAll, sendDraft, undoSend, pendingId]);
  const openDraft = useCallback((draft: Draft) => setActive(draft), []);
  const undoSendItem = useCallback((id: string) => { void undo(id); }, [undo]);
  const composer = active ? (
    <Composer
      key={active.id}
      ref={editor}
      draft={active}
      accounts={accounts}
      onClose={() => { setActive(null); void refresh(); }}
      onQueued={() => { setActive(null); void refresh(); }}
    />
  ) : null;
  return {
    context,
    drafts,
    outbox,
    clock,
    sentCount: outbox.filter((o) => o.state === "sent").length,
    draftCount: drafts.length,
    outboxCount: outbox.filter((o) => !["sent", "canceled"].includes(o.state)).length,
    openDraft,
    undoSendItem,
    restoreFailedSend,
    reconcileSend,
    activeDraft: active,
    composer,
    overlay: <>
      {pending && !active && <div className="send-notice" role="status">{pending.deadline > clock ? `Sending in ${Math.ceil((pending.deadline - clock) / 1000)}s` : "Queued for delivery"}<button onClick={() => void undo(pending.id)}>Undo send</button></div>}
      {closing && <div className="exit-backdrop"><div className="exit-notice" tabIndex={-1} role="dialog" aria-modal="true" aria-label="Closing Dispatch" onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Tab") { e.preventDefault(); e.currentTarget.querySelector("button")?.focus(); } }}><span>Saving drafts and finishing pending delivery before closing… Queued mail remains saved for the next launch.</span>{pending && <button onClick={() => void undo(pending.id)}>Undo queued send</button>}</div></div>}
      {error && <div className="compose-notice" role="alert">{error}<button aria-label="Dismiss compose error" onClick={() => setError("")}><X size={16} /></button></div>}
    </>,
  };
}

export function DraftsList({ drafts, onOpen }: { drafts: Draft[]; onOpen(draft: Draft): void }) {
  if (drafts.length === 0) return <p className="empty">No saved drafts.</p>;
  return <>{drafts.map((d) => (
    <button className="draft-row" key={d.id} onClick={() => onOpen(d)}>
      <strong>{d.subject || "(no subject)"}</strong>
      <span>{d.to || "No recipients"}</span>
      <small>{new Date(d.updatedAt).toLocaleString()} · Saved on this device</small>
    </button>
  ))}</>;
}

export function OutboxList({
  outbox,
  clock,
  onUndo,
  onRestore,
  onReconcile,
}: {
  outbox: OutboxItem[];
  clock: number;
  onUndo(id: string): void;
  onRestore(id: string): void;
  onReconcile(id: string): void;
}) {
  const visible = outbox.filter((o) => o.state !== "canceled");
  if (visible.length === 0) return <p className="empty">No outgoing messages.</p>;
  return <>{visible.map((o) => (
    <article className="outbox-row" key={o.id}>
      <strong>{o.draft.subject || "(no subject)"}</strong>
      <span>From {o.draft.account} · To {o.draft.to || o.draft.cc || "Bcc recipients"}</span>
      <small>{o.state === "undo_pending" ? (o.deadline > clock ? `Undo available · ${Math.ceil((o.deadline - clock) / 1000)}s` : "Waiting for connection") : o.state}</small>
      {o.error && <p>{o.error}</p>}
      {["undo_pending", "ready"].includes(o.state) && <button onClick={() => onUndo(o.id)}>Undo send</button>}
      {o.state === "failed" && <button onClick={() => onRestore(o.id)}>Restore draft</button>}
      {o.state === "uncertain" && <button onClick={() => onReconcile(o.id)}>Check sent mail</button>}
    </article>
  ))}</>;
}
